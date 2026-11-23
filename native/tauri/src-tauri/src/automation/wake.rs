//! MC-39: schedules that fire while the app is closed (앱이 꺼져 있어도 예약 실행, off by default; the pure rules are
//! `mighty_core::automation::wake`).
//!
//! - **One OS wake-up** for the earliest next fire across all schedules, kept in step by the scheduler ([`sync_later`] after each
//!   pass: at the start, after a change of the jobs or the setting, after a fire) and at the quit ([`at_quit`]); removed when the
//!   setting goes off or no enabled schedule is left. Only where the app may change the machine, never in a smoke.
//!   - The Mac: `~/Library/LaunchAgents/<identifier>.schedule[.<profile mark>].plist` ([`LaunchAgentWake`]), written whole (a
//!     temporary file renamed over it, 0644), then `launchctl bootout gui/<uid>/<label>` and `launchctl bootstrap gui/<uid> <file>` so
//!     it counts without a new login; only this label, each call its own time limit, never sudo.
//!   - Windows: the current user's Task Scheduler task `Mighty Code schedule[ <profile mark>]` ([`TaskWake`]), registered with
//!     `schtasks /Create /XML … /F` from a UTF-16 task file (the local start time, `StartWhenAvailable`, no time limit, never
//!     elevated), removed with `schtasks /Delete /F`.
//! - **The start the OS makes** is `--server --for-schedules`: an app already running for the profile ends it at once (MC-3's
//!   handoff). On the Mac the process launchd started hands over to a detached copy of itself and ends ([`leave_launchd`]), so
//!   registering the next wake (which boots the job out) never stops the server doing the work. The server then runs as MC-3's
//!   headless server: the scheduler's catch-up fires what is due (`catchUp`, budgets, records, notifications and phone pushes as
//!   always), and [`watch_idle`] quits it once nothing has run for five minutes (no run, card, queued input, terminal at work, fire
//!   under way, shown window or phone request), the next wake registered first. It stays when it is also the login server
//!   (로그인할 때 창 없이 시작); its quit keeps the daemon's work as `mighty server stop` does (앱을 닫아도 작업 계속).
//! - Triggers are not woken: the GitHub/Linear watches, standing orders and the webhook listener run only while an app process runs
//!   (also while a server the OS started for a schedule runs).
//! - `<data>/automation/wake.json` keeps what is registered, the last failure and the last start by the OS (Settings shows them).

use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use mighty_core::automation::recurrence::LocalZone;
use mighty_core::automation::wake::{self as rules, IdleClock, LastWake, Registration, Step, WakeOutcome, WakeRecord, WakeTime, Wanted};
use mighty_updates::app_update::auto::IdleFacts;

use super::{lock, store};
use crate::host::Host;

/// Each `launchctl` or `schtasks` call's time limit.
const CALL_LIMIT: Duration = Duration::from_secs(15);
/// How often a server started for schedules looks whether it is idle.
const IDLE_LOOK: Duration = Duration::from_secs(15);
/// A phone request this recent keeps the server (the person is using it).
const PHONE_RECENT: Duration = Duration::from_secs(60);

// ── commands ─────────────────────────────────────────────────────────────────────────────────────────────────────────────

/// What a command said.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Ran {
    /// The exit code (None when a signal ended it).
    pub code: Option<i32>,
    /// Its standard output and error, trimmed.
    pub text: String,
}

impl Ran {
    pub fn ok(&self) -> bool {
        self.code == Some(0)
    }

    /// The first lines of what it said, for a message (never more than 300 characters).
    fn said(&self, program: &str) -> String {
        let text: String = self.text.lines().take(3).collect::<Vec<_>>().join(" ").chars().take(300).collect();
        match self.code {
            Some(code) if text.is_empty() => format!("{program} exited with {code}"),
            Some(code) => format!("{program} exited with {code}: {text}"),
            None => format!("{program} was ended"),
        }
    }
}

/// Runs the OS's tools (`launchctl`, `schtasks`); tests give a fake that records the calls.
pub trait Commands: Send + Sync {
    fn run(&self, program: &Path, args: &[String], within: Duration) -> Result<Ran, String>;
}

/// The real runner: the platform's process host (no shell, standard input closed, no window, every handle but the pipes closed),
/// the program by its full path, ended at its time limit; output past 64 KB is a failure.
pub struct SystemCommands;

impl Commands for SystemCommands {
    fn run(&self, program: &Path, args: &[String], within: Duration) -> Result<Ran, String> {
        let request = mighty_platform::SpawnRequest::new(program, args.iter().cloned(), std::env::temp_dir());
        let processes = crate::platform::process_host();
        let captured = tauri::async_runtime::block_on(mighty_platform::capture(processes.as_ref(), request, None, within, 65_536))
            .map_err(|e| format!("{}: {e}", program.display()))?;
        let mut text = String::from_utf8_lossy(&captured.stdout).into_owned();
        text.push_str(&String::from_utf8_lossy(&captured.stderr));
        Ok(Ran { code: Some(captured.exit_code), text: text.trim().to_string() })
    }
}

// ── what the OS starts ───────────────────────────────────────────────────────────────────────────────────────────────────

/// This app's wake: its identifier, exe and `--profile`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WakeItem {
    pub identifier: String,
    pub program: PathBuf,
    pub profile: Option<PathBuf>,
}

impl WakeItem {
    /// The LaunchAgent's label (and its file's name).
    #[cfg_attr(windows, allow(dead_code, reason = "the Mac's store"))]
    pub fn label(&self) -> String {
        match &self.profile {
            None => format!("{}.schedule", self.identifier),
            Some(profile) => format!("{}.schedule.{}", self.identifier, crate::server::login::profile_mark(profile)),
        }
    }

    /// The scheduled task's name.
    #[cfg_attr(not(windows), allow(dead_code, reason = "Windows' store"))]
    pub fn task_name(&self) -> String {
        match &self.profile {
            None => "Mighty Code schedule".into(),
            Some(profile) => format!("Mighty Code schedule {}", crate::server::login::profile_mark(profile)),
        }
    }

    /// The program and its arguments: `--server --for-schedules` (and `--profile`).
    pub fn command(&self) -> Vec<String> {
        let mut command = vec![self.program.to_string_lossy().into_owned()];
        command.extend(arguments(self.profile.as_deref()));
        command
    }
}

/// The arguments of a start for schedules.
pub fn arguments(profile: Option<&Path>) -> Vec<String> {
    let mut args = crate::server::arguments(None);
    args.push(rules::SWITCH.to_string());
    if let Some(profile) = profile {
        args.push("--profile".into());
        args.push(profile.to_string_lossy().into_owned());
    }
    args
}

/// Where the wake is registered.
pub trait WakeStore: Send + Sync {
    /// The label or task name.
    fn name(&self) -> String;
    fn installed(&self) -> bool;
    /// Registers (or replaces) the wake for `command` at `at`.
    fn register(&self, command: &[String], at: WakeTime) -> Result<(), String>;
    fn remove(&self) -> Result<(), String>;
}

/// The Mac's LaunchAgent (`folder` is `~/Library/LaunchAgents`; a test's temporary one).
#[cfg_attr(windows, allow(dead_code, reason = "the Mac's store"))]
pub struct LaunchAgentWake<C: Commands> {
    pub folder: PathBuf,
    pub label: String,
    pub uid: u32,
    pub launchctl: PathBuf,
    pub commands: C,
}

#[cfg_attr(windows, allow(dead_code, reason = "the Mac's store"))]
impl<C: Commands> LaunchAgentWake<C> {
    pub fn file(&self) -> PathBuf {
        self.folder.join(format!("{}.plist", self.label))
    }

    fn launchctl(&self, args: &[String]) -> Result<Ran, String> {
        self.commands.run(&self.launchctl, args, CALL_LIMIT)
    }

    fn bootout(&self) -> Result<Ran, String> {
        self.launchctl(&["bootout".into(), format!("gui/{}/{}", self.uid, self.label)])
    }

    fn bootstrap(&self) -> Result<Ran, String> {
        self.launchctl(&["bootstrap".into(), format!("gui/{}", self.uid), self.file().to_string_lossy().into_owned()])
    }
}

impl<C: Commands> WakeStore for LaunchAgentWake<C> {
    fn name(&self) -> String {
        self.label.clone()
    }

    fn installed(&self) -> bool {
        std::fs::symlink_metadata(self.file()).is_ok_and(|meta| meta.is_file())
    }

    fn register(&self, command: &[String], at: WakeTime) -> Result<(), String> {
        let plist = rules::launch_agent_plist(&self.label, command, at);
        std::fs::create_dir_all(&self.folder).map_err(|e| e.to_string())?;
        let temporary = self.folder.join(format!(".{}.plist.{}.tmp", self.label, std::process::id()));
        let written =
            crate::server::login::write_readable(&temporary, plist.as_bytes()).and_then(|()| std::fs::rename(&temporary, self.file()));
        if let Err(error) = written {
            let _ = std::fs::remove_file(&temporary);
            return Err(error.to_string());
        }
        // The loaded job (if any) out first: launchd reads a job's calendar only when it is loaded. "Not loaded" is not a failure.
        let _ = self.bootout()?;
        let mut done = self.bootstrap()?;
        if !done.ok() {
            // A bootout launchd had not finished yet answers "Input/output error": once more, a moment later.
            std::thread::sleep(Duration::from_millis(500));
            let _ = self.bootout();
            done = self.bootstrap()?;
        }
        if done.ok() { Ok(()) } else { Err(done.said("launchctl bootstrap")) }
    }

    fn remove(&self) -> Result<(), String> {
        let _ = self.bootout()?;
        match std::fs::remove_file(self.file()) {
            Err(error) if error.kind() != io::ErrorKind::NotFound => Err(error.to_string()),
            _ => Ok(()),
        }
    }
}

/// Windows' Task Scheduler (`schtasks` by its full path; `scratch` the task file's folder, the profile's `automation`).
#[cfg_attr(not(windows), allow(dead_code, reason = "Windows' store"))]
pub struct TaskWake<C: Commands> {
    pub name: String,
    pub scratch: PathBuf,
    pub schtasks: PathBuf,
    pub commands: C,
}

#[cfg_attr(not(windows), allow(dead_code, reason = "Windows' store"))]
impl<C: Commands> TaskWake<C> {
    fn schtasks(&self, args: Vec<String>) -> Result<Ran, String> {
        self.commands.run(&self.schtasks, &args, CALL_LIMIT)
    }
}

impl<C: Commands> WakeStore for TaskWake<C> {
    fn name(&self) -> String {
        self.name.clone()
    }

    fn installed(&self) -> bool {
        self.schtasks(rules::schtasks_query(&self.name)).is_ok_and(|ran| ran.ok())
    }

    fn register(&self, command: &[String], at: WakeTime) -> Result<(), String> {
        let (program, arguments) = command.split_first().ok_or("no program")?;
        let xml = rules::task_xml(program, arguments, at);
        std::fs::create_dir_all(&self.scratch).map_err(|e| e.to_string())?;
        let file = self.scratch.join("wake-task.xml");
        std::fs::write(&file, rules::utf16_file(&xml)).map_err(|e| e.to_string())?;
        let done = self.schtasks(rules::schtasks_create(&self.name, &file.to_string_lossy()));
        let _ = std::fs::remove_file(&file);
        let done = done?;
        if done.ok() { Ok(()) } else { Err(done.said("schtasks /Create")) }
    }

    fn remove(&self) -> Result<(), String> {
        let done = self.schtasks(rules::schtasks_delete(&self.name))?;
        if done.ok() || !self.installed() { Ok(()) } else { Err(done.said("schtasks /Delete")) }
    }
}

#[cfg(unix)]
unsafe extern "C" {
    #[link_name = "getuid"]
    fn libc_getuid() -> u32;
}

/// This app's wake item.
pub fn item(host: &Host) -> Option<WakeItem> {
    let program = std::env::current_exe().ok()?;
    Some(WakeItem { identifier: host.app.config().identifier.clone(), program, profile: crate::server::profile() })
}

/// This OS's place for the wake (None where the app has none: Linux).
pub fn os_store(host: &Host, item: &WakeItem) -> Option<Box<dyn WakeStore>> {
    #[cfg(target_os = "macos")]
    {
        let _ = host;
        let home = std::env::var_os("HOME").map(PathBuf::from).filter(|home| home.is_absolute())?;
        // SAFETY: getuid has no preconditions.
        let uid = unsafe { libc_getuid() };
        Some(Box::new(LaunchAgentWake {
            folder: home.join("Library").join("LaunchAgents"),
            label: item.label(),
            uid,
            launchctl: PathBuf::from("/bin/launchctl"),
            commands: SystemCommands,
        }))
    }
    #[cfg(windows)]
    {
        let root =
            std::env::var_os("SystemRoot").map(PathBuf::from).filter(|p| p.is_absolute()).unwrap_or_else(|| PathBuf::from(r"C:\Windows"));
        Some(Box::new(TaskWake {
            name: item.task_name(),
            scratch: store::folder(&host.data_dir),
            schtasks: root.join("System32").join("schtasks.exe"),
            commands: SystemCommands,
        }))
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        let _ = (host, item);
        None
    }
}

/// Whether this OS has a wake (the Mac and Windows).
pub fn supported() -> bool {
    cfg!(any(target_os = "macos", windows))
}

// ── the record and the sync ──────────────────────────────────────────────────────────────────────────────────────────────

/// Why a sync runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Urgency {
    /// The scheduler's pass: a later wake at most every ten minutes.
    Normal,
    /// The app quits: whatever is needed, now.
    Quit,
    /// 지금 다시 등록: registered again even when it looks right.
    Again,
}

#[derive(Default)]
struct Wake {
    record: Mutex<Option<WakeRecord>>,
    last_try: Mutex<Option<DateTime<Utc>>>,
    /// One sync at a time.
    syncing: Mutex<()>,
    /// Fires the scheduler started that have not finished.
    firing: AtomicUsize,
    /// The scheduler's first pass (the catch-up) is done.
    first_pass: AtomicBool,
    /// This process stays under launchd (its hand-over failed): it registers nothing (a bootout would end it).
    under_launchd: AtomicBool,
    /// The OS's item was looked for once in this process.
    looked: AtomicBool,
}

fn wake() -> &'static Wake {
    static WAKE: OnceLock<Wake> = OnceLock::new();
    WAKE.get_or_init(Wake::default)
}

fn path(data_dir: &Path) -> PathBuf {
    store::folder(data_dir).join("wake.json")
}

fn load_record(data_dir: &Path) -> WakeRecord {
    std::fs::read(path(data_dir)).ok().and_then(|bytes| serde_json::from_slice(&bytes).ok()).unwrap_or_default()
}

/// The record as last read or written.
pub fn record(host: &Host) -> WakeRecord {
    lock(&wake().record).get_or_insert_with(|| load_record(&host.data_dir)).clone()
}

/// Changes the record in memory (one change at a time) and in its file where the app writes its profile.
fn change_record(host: &Host, change: impl FnOnce(&mut WakeRecord)) {
    let current = {
        let mut slot = lock(&wake().record);
        let record = slot.get_or_insert_with(|| load_record(&host.data_dir));
        change(record);
        record.clone()
    };
    if host.writes()
        && let Ok(bytes) = serde_json::to_vec_pretty(&current)
    {
        let _ = mighty_core::automation::write_atomic(&path(&host.data_dir), &bytes);
    }
    crate::native::touch(host, crate::settings::SURFACE);
}

/// What one sync did to the record: the registration made or removed, or the failure.
pub fn apply(record: &mut WakeRecord, step: &Step, name: &str, outcome: Result<(), String>, now: DateTime<Utc>) {
    match (step, outcome) {
        (Step::Nothing, _) => {}
        (_, Err(error)) => record.error = Some(error),
        (Step::Register(wanted), Ok(())) => {
            record.registration =
                Some(Registration { at: wanted.at, name: name.to_string(), command: wanted.command.clone(), registered_at: now });
            record.error = None;
        }
        (Step::Remove, Ok(())) => {
            record.registration = None;
            record.error = None;
        }
    }
}

/// One sync against `store` (`installed`: whether its item is there): what [`rules::decide`] says, done. The step taken.
pub fn sync_with(
    store: &dyn WakeStore,
    record: &mut WakeRecord,
    wanted: Option<Wanted>,
    installed: bool,
    now: DateTime<Utc>,
    last_try: &mut Option<DateTime<Utc>>,
    urgency: Urgency,
) -> Step {
    let forced = urgency == Urgency::Again;
    let quitting = urgency == Urgency::Quit;
    let mut step = rules::decide(wanted.clone(), record.registration.as_ref(), installed, now, *last_try, forced || quitting);
    // The quit only lifts the ten-minute hold; it does not register again what is already right.
    if quitting
        && let (Step::Register(want), Some(current)) = (&step, record.registration.as_ref())
        && installed
        && current.command == want.command
        && current.at.timestamp() / 60 == want.at.timestamp() / 60
    {
        step = Step::Nothing;
    }
    let outcome = match &step {
        Step::Nothing => Ok(()),
        Step::Register(want) => {
            *last_try = Some(now);
            store.register(&want.command, WakeTime::of(want.at, &LocalZone))
        }
        Step::Remove => store.remove(),
    };
    apply(record, &step, &store.name(), outcome, now);
    step
}

/// Whether the person turned the wake on.
pub fn enabled() -> bool {
    lock(&super::automation().settings).wake_when_closed
}

/// One sync now (on the calling thread: it may run `launchctl` or `schtasks`). The OS's item is looked for the first time in this
/// process and on 지금 다시 등록; otherwise the record is trusted (no `schtasks /Query` on every pass or quit).
pub fn sync(host: &Host, next_fire: Option<DateTime<Utc>>, urgency: Urgency) {
    if host.smoke.is_some() || !supported() || !host.may_change_machine() || wake().under_launchd.load(Ordering::SeqCst) {
        return;
    }
    let _one = lock(&wake().syncing);
    let Some(item) = item(host) else { return };
    let wanted = rules::wanted(enabled(), true, next_fire, item.command());
    let mut current = record(host);
    let looked = wake().looked.swap(true, Ordering::SeqCst);
    let trust = looked && urgency != Urgency::Again;
    // The setting off and nothing registered (most people): no OS call after the first look.
    if trust && wanted.is_none() && current.registration.is_none() {
        return;
    }
    let Some(store) = os_store(host, &item) else { return };
    let installed = if trust { current.registration.is_some() } else { store.installed() };
    let mut last_try = *lock(&wake().last_try);
    let before = current.clone();
    let step = sync_with(store.as_ref(), &mut current, wanted, installed, Utc::now(), &mut last_try, urgency);
    *lock(&wake().last_try) = last_try;
    let failure = current.error.as_deref().filter(|_| current.error != before.error).map(|e| format!(" (failed: {e})")).unwrap_or_default();
    match &step {
        Step::Nothing => {}
        Step::Register(want) => host.log.line(&format!("automation: OS wake {} for {}{failure}", store.name(), want.at.to_rfc3339())),
        Step::Remove => host.log.line(&format!("automation: OS wake {} removed{failure}", store.name())),
    }
    if current != before {
        // Only what the sync owns: the last start's words may have moved meanwhile.
        change_record(host, |record| {
            record.registration = current.registration;
            record.error = current.error;
        });
    }
}

/// The scheduler's pass: a sync on a blocking task (one at a time).
pub fn sync_later(host: &Arc<Host>, next_fire: Option<DateTime<Utc>>, urgency: Urgency) {
    if host.smoke.is_some() || !supported() || !host.may_change_machine() {
        return;
    }
    let host = host.clone();
    tauri::async_runtime::spawn_blocking(move || sync(&host, next_fire, urgency));
}

/// The app quits: the wake left right for the next fire (a later one registered without waiting).
pub fn at_quit(host: &Host) {
    if crate::server::started_for_schedules() {
        change_record(host, |record| {
            if let Some(last) = record.last_wake.as_mut().filter(|last| last.outcome == WakeOutcome::Running) {
                last.outcome = WakeOutcome::Stopped;
                last.ended = Some(Utc::now());
                let (fired, skipped) = counted_since(last.started);
                (last.fired, last.skipped) = (fired, skipped);
            }
        });
    }
    sync(host, super::next_wake(), Urgency::Quit);
}

/// 지금 다시 등록.
pub fn register_again(host: &Arc<Host>) {
    let host = host.clone();
    tauri::async_runtime::spawn_blocking(move || sync(&host, super::next_wake(), Urgency::Again));
}

/// The switch: saved with the automation settings; the scheduler's next pass registers or removes.
pub fn set_enabled(host: &Arc<Host>, on: bool) {
    lock(&super::automation().settings).wake_when_closed = on;
    super::settings_changed(host);
}

// ── the scheduler's marks ────────────────────────────────────────────────────────────────────────────────────────────────

/// A fire under way (held until it has made its pane and sent its prompt).
pub struct Firing;

impl Firing {
    pub fn begin() -> Firing {
        wake().firing.fetch_add(1, Ordering::SeqCst);
        Firing
    }
}

impl Drop for Firing {
    fn drop(&mut self) {
        wake().firing.fetch_sub(1, Ordering::SeqCst);
    }
}

/// The scheduler finished its first pass (the catch-up's fires have started).
pub fn first_pass_done() {
    wake().first_pass.store(true, Ordering::SeqCst);
}

/// The records made since `since`: (fired, skipped).
fn counted_since(since: DateTime<Utc>) -> (u32, u32) {
    let since = since.to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    let records = super::recent(None, 300);
    let made = records.iter().filter(|r| r.fired_at.as_str() >= since.as_str());
    let (skipped, fired): (Vec<_>, Vec<_>) = made.partition(|r| r.state == mighty_core::automation::records::RecordState::Skipped);
    (u32::try_from(fired.len()).unwrap_or(u32::MAX), u32::try_from(skipped.len()).unwrap_or(u32::MAX))
}

// ── the server the OS started ────────────────────────────────────────────────────────────────────────────────────────────

/// What keeps a server started for schedules running.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Activity {
    /// The app's work, as the update's idle check reads it (runs, cards, queued inputs, drafts, terminals at work, CLI work, phone
    /// uploads, runs hosted for another computer, a dialog or Settings open, an agent active in the last three minutes).
    pub facts: IdleFacts,
    pub window_shown: bool,
    /// A phone's request under way or in the last minute.
    pub phone: bool,
    /// Fires under way, or the scheduler's first pass not done yet.
    pub firing: bool,
}

/// Whether anything keeps the server running. Workspace sharing alone does not (it is a setting, not work), nor the person's input
/// elsewhere on the computer.
pub fn busy(activity: &Activity, now: DateTime<Utc>) -> bool {
    use mighty_updates::app_update::auto::{IdleBlocker, idle_blockers};
    let facts = IdleFacts { sharing_workspaces: false, last_user_input: None, ..activity.facts.clone() };
    let work = idle_blockers(&facts, now).into_iter().any(|blocker| blocker != IdleBlocker::UserActive);
    work || activity.window_shown || activity.phone || activity.firing
}

fn activity(host: &Arc<Host>) -> Activity {
    Activity {
        facts: crate::app_update::work_facts(host),
        window_shown: crate::server::window_shown(host),
        phone: crate::phone::requests_recent(PHONE_RECENT),
        firing: wake().firing.load(Ordering::SeqCst) > 0 || !wake().first_pass.load(Ordering::SeqCst),
    }
}

/// A server started for schedules: its start recorded, then it quits once idle for five minutes (its next wake registered first),
/// unless it is also the login server.
pub fn watch_idle(host: &Arc<Host>) {
    if !crate::server::started_for_schedules() || host.smoke.is_some() {
        return;
    }
    let started = Utc::now();
    change_record(host, |record| {
        record.last_wake = Some(LastWake { started, ended: None, fired: 0, skipped: 0, outcome: WakeOutcome::Running });
    });
    host.log.line("automation: started by the OS for schedules");
    let weak = Arc::downgrade(host);
    tauri::async_runtime::spawn(async move {
        let mut clock = IdleClock::default();
        loop {
            tokio::time::sleep(IDLE_LOOK).await;
            let Some(host) = weak.upgrade() else { return };
            if host.closing() {
                return;
            }
            let login = {
                let host = host.clone();
                tauri::async_runtime::spawn_blocking(move || crate::server::login::installed(&host)).await.unwrap_or(false)
            };
            if !rules::quits_when_idle(true, login) {
                host.log.line("automation: the login server too; it keeps running");
                change_record(&host, |record| {
                    if let Some(last) = record.last_wake.as_mut() {
                        last.outcome = WakeOutcome::Stayed;
                    }
                });
                return;
            }
            let now = Utc::now();
            if !clock.tick(Instant::now(), busy(&activity(&host), now)) {
                continue;
            }
            host.log.line("automation: nothing has run for five minutes; the server started for schedules quits");
            let (fired, skipped) = counted_since(started);
            change_record(&host, |record| {
                if let Some(last) = record.last_wake.as_mut() {
                    *last = LastWake { started, ended: Some(Utc::now()), fired, skipped, outcome: WakeOutcome::Idle };
                }
            });
            let quitting = host.clone();
            let _ = tauri::async_runtime::spawn_blocking(move || sync(&quitting, super::next_wake(), Urgency::Quit)).await;
            crate::server::stop(&host, false, "idle after its schedules");
            return;
        }
    });
}

/// main.rs, after the handoff: the Mac's process launchd started for a schedule hands over to a detached copy of itself (same
/// arguments, the environment without launchd's mark) and ends, so the next registration's `bootout` never ends the server. Its exit
/// code, None to go on here (not launchd's start, or the copy could not start: then this process registers nothing).
pub fn leave_launchd(server: bool, for_schedules: bool, profile: Option<&Path>) -> Option<i32> {
    if !hands_over(cfg!(target_os = "macos"), server, for_schedules, std::env::var_os(rules::LAUNCHD_MARK).is_some()) {
        return None;
    }
    let environment: std::collections::BTreeMap<String, String> = std::env::vars_os()
        .filter_map(|(k, v)| Some((k.into_string().ok()?, v.into_string().ok()?)))
        .filter(|(k, _)| k != rules::LAUNCHD_MARK)
        .collect();
    match crate::server::launch_detached_with(arguments(profile), mighty_platform::DetachedEnvironment::Exactly(environment)) {
        Ok(pid) => {
            eprintln!("Mighty Code starts for its schedules (pid {pid}).");
            Some(0)
        }
        Err(error) => {
            eprintln!("Mighty Code could not leave launchd ({error}); it runs here and registers no wake.");
            wake().under_launchd.store(true, Ordering::SeqCst);
            None
        }
    }
}

/// A time as Settings shows it: local, to the minute.
fn shown_time(at: DateTime<Utc>) -> String {
    at.with_timezone(&chrono::Local).format("%m-%d %H:%M").to_string()
}

/// Whether this start hands over: the Mac's `--server --for-schedules` carrying launchd's mark.
fn hands_over(mac: bool, server: bool, for_schedules: bool, marked: bool) -> bool {
    mac && server && for_schedules && marked
}

/// Settings' words for the wake: registered for when, or why not.
pub fn state_words(host: &Host) -> String {
    let l = host.locale;
    if !supported() {
        return l.get("automation.wake.unsupported");
    }
    let current = record(host);
    match (&current.registration, enabled()) {
        (Some(registration), _) => l.get_with("automation.wake.registered", &[("at", &shown_time(registration.at))]),
        (None, false) => l.get("automation.wake.off"),
        (None, true) if super::next_wake().is_none() => l.get("automation.wake.noSchedules"),
        (None, true) => l.get("automation.wake.notRegistered"),
    }
}

/// Settings' words for the last start by the OS.
pub fn last_words(host: &Host) -> Option<String> {
    let l = host.locale;
    let last = record(host).last_wake?;
    let counts = [("fired", last.fired.to_string()), ("skipped", last.skipped.to_string())];
    let counts: Vec<(&str, &str)> = counts.iter().map(|(k, v)| (*k, v.as_str())).collect();
    let result = match last.outcome {
        WakeOutcome::Running => l.get("automation.wake.result.running"),
        WakeOutcome::Idle => l.get_with("automation.wake.result.idle", &counts),
        WakeOutcome::Stayed => l.get("automation.wake.result.stayed"),
        WakeOutcome::Stopped => l.get_with("automation.wake.result.stopped", &counts),
    };
    Some(l.get_with("automation.wake.last", &[("at", &shown_time(last.started)), ("result", &result)]))
}

/// Settings' words for the last failure.
pub fn error_words(host: &Host) -> Option<String> {
    record(host).error.map(|error| host.locale.get_with("automation.wake.failed", &[("reason", &error)]))
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex as StdMutex;

    use chrono::TimeZone as _;

    use super::*;

    /// Records each call and answers from a script (by the first argument): `launchctl` and `schtasks` are never run.
    #[derive(Default)]
    struct Fake {
        calls: StdMutex<Vec<Vec<String>>>,
        /// First argument → exit code (0 unless named).
        codes: StdMutex<std::collections::HashMap<String, Vec<i32>>>,
    }

    impl Fake {
        fn answer(&self, verb: &str, codes: &[i32]) {
            lock(&self.codes).insert(verb.into(), codes.to_vec());
        }
        fn calls(&self) -> Vec<Vec<String>> {
            lock(&self.calls).clone()
        }
    }

    impl Commands for &Fake {
        fn run(&self, program: &Path, args: &[String], within: Duration) -> Result<Ran, String> {
            assert_eq!(within, CALL_LIMIT);
            let mut call = vec![program.to_string_lossy().into_owned()];
            call.extend(args.iter().cloned());
            lock(&self.calls).push(call);
            let mut codes = lock(&self.codes);
            let code = codes
                .get_mut(&args[0])
                .and_then(|codes| if codes.len() > 1 { Some(codes.remove(0)) } else { codes.first().copied() })
                .unwrap_or(0);
            Ok(Ran { code: Some(code), text: if code == 0 { String::new() } else { format!("{} failed", args[0]) } })
        }
    }

    fn temp(name: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        std::env::temp_dir().join(format!("mighty-wake-{name}-{}-{nanos}", std::process::id()))
    }

    fn mac_item(profile: Option<&str>) -> WakeItem {
        WakeItem {
            identifier: "dev.mightycode.native".into(),
            program: PathBuf::from("/Applications/Mighty Code.app/Contents/MacOS/MightyCode"),
            profile: profile.map(PathBuf::from),
        }
    }

    fn utc(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(y, mo, d, h, mi, 0).unwrap()
    }

    #[test]
    fn the_item_names_its_label_task_and_command() {
        let item = mac_item(None);
        assert_eq!(item.label(), "dev.mightycode.native.schedule");
        assert_eq!(item.task_name(), "Mighty Code schedule");
        assert_eq!(item.command(), ["/Applications/Mighty Code.app/Contents/MacOS/MightyCode", "--server", "--for-schedules"]);
        let profiled = mac_item(Some("/Users/me/p one"));
        let mark = crate::server::login::profile_mark(Path::new("/Users/me/p one"));
        assert_eq!(profiled.label(), format!("dev.mightycode.native.schedule.{mark}"));
        assert_eq!(profiled.task_name(), format!("Mighty Code schedule {mark}"));
        assert_eq!(profiled.command()[1..], ["--server", "--for-schedules", "--profile", "/Users/me/p one"]);
        let login = crate::server::login::LoginItem { identifier: item.identifier.clone(), program: item.program.clone(), profile: None };
        assert_ne!(item.label(), login.label(), "never the login item's label");
    }

    #[test]
    fn the_launch_agent_is_written_whole_then_booted_out_and_in() {
        let root = temp("launchd");
        let fake = Fake::default();
        let store = LaunchAgentWake {
            folder: root.join("Library").join("LaunchAgents"),
            label: "dev.mightycode.native.schedule".into(),
            uid: 501,
            launchctl: PathBuf::from("/bin/launchctl"),
            commands: &fake,
        };
        assert!(!store.installed());
        let at = WakeTime { year: 2026, month: 11, day: 24, hour: 9, minute: 0 };
        let command = mac_item(None).command();
        store.register(&command, at).unwrap();
        assert!(store.installed());
        let file = store.file();
        assert_eq!(std::fs::read_to_string(&file).unwrap(), rules::launch_agent_plist(&store.label, &command, at));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            assert_eq!(std::fs::metadata(&file).unwrap().permissions().mode() & 0o777, 0o644);
        }
        let plist_path = file.to_string_lossy().into_owned();
        assert_eq!(
            fake.calls(),
            vec![
                vec!["/bin/launchctl".to_string(), "bootout".into(), "gui/501/dev.mightycode.native.schedule".into()],
                vec!["/bin/launchctl".to_string(), "bootstrap".into(), "gui/501".into(), plist_path.clone()],
            ],
            "only this label, the file by its path (no shell)"
        );
        let names: Vec<_> = std::fs::read_dir(&store.folder).unwrap().map(|e| e.unwrap().file_name()).collect();
        assert_eq!(names.len(), 1, "no temporary file left: {names:?}");
        // A bootstrap that fails once (launchd still booting the old job out) is tried again; twice is an error.
        fake.answer("bootstrap", &[5, 0]);
        store.register(&command, at).unwrap();
        fake.answer("bootstrap", &[5]);
        let error = store.register(&command, at).unwrap_err();
        assert!(error.contains("launchctl bootstrap exited with 5"), "{error}");
        store.remove().unwrap();
        assert!(!store.installed());
        assert_eq!(fake.calls().last().unwrap()[1], "bootout");
        store.remove().unwrap();
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn the_scheduled_task_is_created_from_its_file_and_deleted() {
        let root = temp("schtasks");
        let fake = Fake::default();
        let store = TaskWake {
            name: "Mighty Code schedule".into(),
            scratch: root.clone(),
            schtasks: PathBuf::from(r"C:\Windows\System32\schtasks.exe"),
            commands: &fake,
        };
        let command: Vec<String> =
            [r"C:\Program Files\Mighty Code\MightyCode.exe", "--server", "--for-schedules", "--profile", r"D:\my work\"]
                .map(String::from)
                .to_vec();
        store.register(&command, WakeTime { year: 2026, month: 11, day: 24, hour: 9, minute: 0 }).unwrap();
        let file = root.join("wake-task.xml").to_string_lossy().into_owned();
        assert_eq!(
            fake.calls(),
            vec![vec![
                r"C:\Windows\System32\schtasks.exe".to_string(),
                "/Create".into(),
                "/TN".into(),
                "Mighty Code schedule".into(),
                "/XML".into(),
                file,
                "/F".into()
            ]]
        );
        assert!(!root.join("wake-task.xml").exists(), "the task file is removed once read");
        fake.answer("/Create", &[1]);
        assert!(
            store
                .register(&command, WakeTime { year: 2026, month: 11, day: 24, hour: 9, minute: 0 })
                .unwrap_err()
                .contains("schtasks /Create exited with 1")
        );
        // Deleted; a task already gone is not a failure.
        store.remove().unwrap();
        fake.answer("/Delete", &[1]);
        fake.answer("/Query", &[1]);
        store.remove().unwrap();
        fake.answer("/Query", &[0]);
        assert!(store.remove().is_err(), "still there: a failure");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The register/unregister decisions against a store: the setting off, no schedule, after a fire, the quit, 지금 다시 등록.
    #[test]
    fn a_sync_registers_moves_and_removes_the_wake() {
        let root = temp("sync");
        let fake = Fake::default();
        let store = LaunchAgentWake {
            folder: root.clone(),
            label: "dev.mightycode.native.schedule".into(),
            uid: 501,
            launchctl: PathBuf::from("/bin/launchctl"),
            commands: &fake,
        };
        let command = mac_item(None).command();
        let mut record = WakeRecord::default();
        let mut last_try = None;
        let now = utc(2026, 11, 23, 15, 0);
        let nine = utc(2026, 11, 24, 14, 0);
        // The setting off and nothing there: no call at all.
        assert_eq!(sync_with(&store, &mut record, None, store.installed(), now, &mut last_try, Urgency::Normal), Step::Nothing);
        assert!(fake.calls().is_empty());
        // On with a schedule: registered.
        let wants = |at| Some(Wanted { at, command: command.clone() });
        assert!(matches!(
            sync_with(&store, &mut record, wants(nine), store.installed(), now, &mut last_try, Urgency::Normal),
            Step::Register(_)
        ));
        assert_eq!(record.registration.as_ref().map(|r| (r.at, r.name.as_str())), Some((nine, "dev.mightycode.native.schedule")));
        assert!(std::fs::read_to_string(store.file()).unwrap().contains("<integer>24</integer>"));
        let calls = fake.calls().len();
        // The next pass with nothing changed: nothing.
        assert_eq!(
            sync_with(
                &store,
                &mut record,
                wants(nine),
                store.installed(),
                now + chrono::Duration::minutes(1),
                &mut last_try,
                Urgency::Normal
            ),
            Step::Nothing
        );
        assert_eq!(fake.calls().len(), calls);
        // The quit with it right: nothing either; 지금 다시 등록: again.
        assert_eq!(sync_with(&store, &mut record, wants(nine), store.installed(), now, &mut last_try, Urgency::Quit), Step::Nothing);
        assert!(matches!(
            sync_with(&store, &mut record, wants(nine), store.installed(), now, &mut last_try, Urgency::Again),
            Step::Register(_)
        ));
        // After the 09:00 fire: tomorrow's.
        let after = nine + chrono::Duration::minutes(1);
        let tomorrow = utc(2026, 11, 25, 14, 0);
        assert!(matches!(
            sync_with(&store, &mut record, wants(tomorrow), store.installed(), after, &mut last_try, Urgency::Normal),
            Step::Register(_)
        ));
        assert_eq!(record.registration.as_ref().unwrap().at, tomorrow);
        // A failure is kept for Settings, and the record keeps what was there.
        fake.answer("bootstrap", &[5]);
        let sooner = utc(2026, 11, 25, 13, 0);
        sync_with(&store, &mut record, wants(sooner), store.installed(), after, &mut last_try, Urgency::Normal);
        assert!(record.error.as_deref().is_some_and(|e| e.contains("bootstrap")));
        assert_eq!(record.registration.as_ref().unwrap().at, tomorrow);
        fake.answer("bootstrap", &[0]);
        sync_with(&store, &mut record, wants(sooner), store.installed(), after, &mut last_try, Urgency::Normal);
        assert_eq!((record.error.clone(), record.registration.as_ref().unwrap().at), (None, sooner));
        // The setting off (or no enabled schedule left): removed.
        assert_eq!(sync_with(&store, &mut record, None, store.installed(), after, &mut last_try, Urgency::Normal), Step::Remove);
        assert!(!store.installed() && record.registration.is_none());
        assert_eq!(sync_with(&store, &mut record, None, store.installed(), after, &mut last_try, Urgency::Normal), Step::Nothing);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn the_server_started_for_schedules_stays_only_while_something_runs() {
        let now = utc(2026, 11, 24, 14, 10);
        let quiet = Activity::default();
        assert!(!busy(&quiet, now));
        for (what, activity) in [
            ("a run", Activity { facts: IdleFacts { agent_runs: 1, ..IdleFacts::default() }, ..Activity::default() }),
            ("a card", Activity { facts: IdleFacts { pending_cards: 1, ..IdleFacts::default() }, ..Activity::default() }),
            ("a terminal", Activity { facts: IdleFacts { terminals: 1, ..IdleFacts::default() }, ..Activity::default() }),
            ("a queued prompt", Activity { facts: IdleFacts { queued_inputs: 1, ..IdleFacts::default() }, ..Activity::default() }),
            (
                "an agent a minute ago",
                Activity {
                    facts: IdleFacts { last_agent_activity: Some(now - chrono::Duration::minutes(1)), ..IdleFacts::default() },
                    ..Activity::default()
                },
            ),
            ("the window", Activity { window_shown: true, ..Activity::default() }),
            ("a phone", Activity { phone: true, ..Activity::default() }),
            ("a fire", Activity { firing: true, ..Activity::default() }),
        ] {
            assert!(busy(&activity, now), "{what}");
        }
        // The person typing in another app, or workspace sharing switched on, keeps nothing running.
        let elsewhere = Activity {
            facts: IdleFacts { last_user_input: Some(now), sharing_workspaces: true, ..IdleFacts::default() },
            ..Activity::default()
        };
        assert!(!busy(&elsewhere, now));
        // With the fake clock: a run, then five quiet minutes.
        let start = Instant::now();
        let mut clock = IdleClock::default();
        assert!(!clock.tick(start, busy(&Activity { firing: true, ..Activity::default() }, now)));
        assert!(!clock.tick(start + Duration::from_secs(60), busy(&quiet, now)));
        assert!(!clock.tick(start + Duration::from_secs(359), busy(&quiet, now)));
        assert!(clock.tick(start + Duration::from_secs(360), busy(&quiet, now)));
    }

    #[test]
    fn the_start_for_schedules_reads_its_arguments() {
        assert_eq!(arguments(None), ["--server", "--for-schedules"]);
        let profile = if cfg!(windows) { r"C:\p" } else { "/p" };
        assert_eq!(arguments(Some(Path::new(profile))), ["--server", "--for-schedules", "--profile", profile]);
        let options = crate::args::parse(arguments(Some(Path::new(profile)))).unwrap();
        assert!(options.server && options.for_schedules);
        assert_eq!(crate::server::handoff::launch_of(&options), crate::server::handoff::Launch::Server, "an app already running ends it");
        // Only the Mac's start by launchd (its mark set) hands over to a detached copy; the copy (mark removed) goes on.
        assert!(hands_over(true, true, true, true));
        assert!(!hands_over(true, true, true, false), "the detached copy, or a start from a terminal");
        assert!(!hands_over(false, true, true, true), "Windows' Task Scheduler start goes on in place");
        assert!(!hands_over(true, true, false, true));
    }
}
