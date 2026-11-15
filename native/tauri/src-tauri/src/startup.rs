//! What runs before the window (W9; C# `WinUI\Program.cs:158-205`): the user's environment taken back after a bare start, the write
//! policy, and the pre-UI migration of the data an app from before the rename left. Owners: `repair_environment` W9-B; the rest W9-F.
//!
//! The write policy (plan §4, D3) replaces W6's single input to the gates (`--allow-state-write`): a release-stamped build run from its
//! install folder saves the state and may change the machine by default, any other build only with `--allow-state-write`, and
//! `--read-only` turns writing off in any build. A release-stamped exe started from anywhere else (a CI candidate opened from
//! Downloads beside the installed C# app) writes nothing unless asked (W9 review M2). A smoke is unchanged: it writes only with
//! `--allow-state-write` in any build (so `smoke.ps1 -Profile` stays read-only on a packaged release exe), and never changes the machine
//! whatever the policy (`Host::may_change_machine`). The gates keep their bodies; only their input moves here: the store gets
//! `policy.writes`, `Host::writes` reads the store, and every gate (`may_change_machine`, `may_listen`, `may_reach_relay`,
//! `may_touch_repo`, `delegation_live`, `AutomaticUpdatePolicy`) reads that.
//!
//! Also here: where a run's log goes (a run that writes nothing keeps its log out of the profile), whether a run adopts C#'s WebView2
//! data, and the page's retry while another process still holds that data with other options.

use std::collections::HashMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use mighty_core::rename_migration::{self, RegistryStore};
use mighty_platform::Paths;

use crate::args::Options;
use crate::build_info;

/// Whether this run writes its profile (and, outside a smoke, may change the machine: `Host::may_change_machine` and the gates over it).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct WritePolicy {
    pub writes: bool,
    /// A release-stamped build running from its install folder: the app the machine-wide registrations name (the toast activator's
    /// `LocalServer32`, W9 review L1).
    pub installed_release: bool,
}

/// The policy this run took, with whether it is on the default profile ([`write_policy`] keeps it; [`owns_registrations`] reads it).
static RUN: OnceLock<(WritePolicy, bool)> = OnceLock::new();

/// The policy for this run: this build's stamp ([`build_info::RELEASE`]) and where it runs from ([`installed`]) over the options
/// ([`policy`]). Kept for [`owns_registrations`].
pub fn write_policy(options: &Options) -> WritePolicy {
    let installed_release = build_info::RELEASE && installed();
    let policy = policy(installed_release, options);
    let _ = RUN.set((policy, options.profile.is_none()));
    policy
}

/// Plan D3's rule with review M2's condition: `--read-only` never writes; `--allow-state-write` writes in any build; otherwise only a
/// release build running from its install folder writes, and not in a smoke.
fn policy(installed_release: bool, options: &Options) -> WritePolicy {
    let writes = !options.read_only && (options.allow_state_write || (installed_release && !options.smoke_test));
    WritePolicy { writes, installed_release }
}

/// Whether this exe runs from the install folder the setup program makes (`MightyCode.iss` `DefaultDirName={userpf}\MightyCode`:
/// `%LOCALAPPDATA%\Programs\MightyCode`, the known folder's, never an environment variable), compared canonically. Off Windows no
/// install folder is named yet (the Mac stage's): never.
fn installed() -> bool {
    #[cfg(windows)]
    {
        let exe = std::env::current_exe().ok();
        let folder = mighty_platform_windows::install_folder();
        matches!((exe.as_deref().and_then(Path::parent), folder), (Some(exe), Some(folder)) if same_folder(exe, &folder))
    }
    #[cfg(not(windows))]
    {
        false
    }
}

/// Two folders the file system names the same (both canonical: case, 8.3 names, links); a folder that does not exist is no match.
fn same_folder(one: &Path, other: &Path) -> bool {
    matches!((std::fs::canonicalize(one), std::fs::canonicalize(other)), (Ok(one), Ok(other)) if one == other)
}

/// Whether this run is the installed release on the default profile, the one app the machine-wide registrations may name (the toast
/// activator's class and its `LocalServer32`; W9 review L1). Other writing runs (a development build with the flag, `--profile`) may
/// serve the class but never register it.
pub fn owns_registrations() -> bool {
    RUN.get().is_some_and(|(policy, default_profile)| policy.installed_release && policy.writes && *default_profile)
}

/// Where this run's log goes (`host.log`, `unimplemented.json`, the smoke's records): `--log-dir`; else the profile's `tauri-logs` when
/// the run writes the profile or is a smoke (whose profile is its own, and where smoke.ps1 finds it); else this machine's local folder
/// for the profile (`<local>\logs\<profile>`), so a run that writes nothing never makes `%APPDATA%\MightyCode` (whose presence would
/// also stop a later start's RenameMigration from moving `MightyClaudeNative` there).
pub fn log_dir(options: &Options, paths: &dyn Paths, policy: WritePolicy) -> PathBuf {
    match &options.log_dir {
        Some(folder) => folder.clone(),
        None if policy.writes || options.smoke_test => paths.logs_dir(),
        None => paths.local_dir().join("logs").join(paths.profile_key()),
    }
}

/// Whether the page and the browser panes adopt C#'s web data (`Paths::web_folders`): a release build on the default profile, only when
/// the run writes (W9 review M1: a `--read-only` run, or a release candidate run beside C#, keeps its own folders, never a folder C#'s
/// WebView2 may still hold).
pub fn adopts_csharp_web_data(options: &Options, policy: WritePolicy) -> bool {
    adopts(build_info::RELEASE, options, policy)
}

fn adopts(release: bool, options: &Options, policy: WritePolicy) -> bool {
    release && options.profile.is_none() && policy.writes
}

/// C#'s browser arguments for the page in its adopted folder: none (C# `new CoreWebView2EnvironmentOptions()`), so an environment C#'s
/// WebView2 left there opens with the same options.
pub const CSHARP_BROWSER_ARGUMENTS: &str = "";

/// The waits between attempts to make the page while its WebView2 data is held by another process with other options (about 10 s).
pub const WEBVIEW_IN_USE_WAITS: [Duration; 6] = [
    Duration::from_millis(500),
    Duration::from_millis(1000),
    Duration::from_millis(1500),
    Duration::from_millis(2000),
    Duration::from_millis(2500),
    Duration::from_millis(2500),
];

/// WebView2's `HRESULT_FROM_WIN32(ERROR_INVALID_STATE)` (0x8007139F): a browser process of another app (C#'s, right after the update
/// swap) still holds the data folder with other environment options.
fn webview_in_use(error: &str) -> bool {
    let error = error.to_ascii_uppercase();
    error.contains("8007139F") || error.contains("-2147019873")
}

/// Makes the page, again after each of [`WEBVIEW_IN_USE_WAITS`] while its data folder is held (W9 review M1); any other error, or the
/// last, is returned. `sleep` waits (the real one blocks the setup, before the event loop runs).
pub fn retry_while_held<T, E: fmt::Display + fmt::Debug>(
    mut attempt: impl FnMut() -> Result<T, E>,
    mut sleep: impl FnMut(Duration),
    log: impl Fn(&str),
) -> Result<T, E> {
    let mut waits = WEBVIEW_IN_USE_WAITS.iter();
    loop {
        match attempt() {
            Ok(made) => return Ok(made),
            Err(error) => {
                let held = webview_in_use(&format!("{error} {error:?}"));
                match waits.next() {
                    Some(wait) if held => {
                        log(&format!("webview: the data folder is held by another process ({error}); again in {} ms", wait.as_millis()));
                        sleep(*wait);
                    }
                    _ => return Err(error),
                }
            }
        }
    }
}

/// C# `UserEnvironment.RepairCurrentProcess` (Program.cs:161): an app started from a bare environment (only `SystemRoot`, as an update
/// helper from before the fix started it) takes the user's own variables back before anything runs — every missing or empty one from
/// `CreateEnvironmentBlock` (the platform's `UpdateInstaller::fresh_environment`; none on the Mac), none present overridden (the
/// decisions are mighty-updates' `app_update::environment`). Returns how many it added.
pub fn repair_environment() -> usize {
    use mighty_updates::app_update::environment;
    let current = |name: &str| std::env::var_os(name).map(|value| value.to_string_lossy().into_owned());
    if !environment::is_bare(&current) {
        return 0;
    }
    let Some(fresh) = crate::platform::update_installer().fresh_environment() else { return 0 };
    let missing = environment::missing(&current, &fresh);
    for (name, value) in &missing {
        // SAFETY: main.rs calls this first thing after the exe's other modes, before Tauri, the runtime, the log or any port has started
        // a thread, so nothing else reads or writes the environment while it changes (what `std::env::set_var` asks of its caller).
        unsafe { std::env::set_var(name, value) };
    }
    missing.len()
}

/// Whether RenameMigration runs (C# Program.cs:169: the default profile outside a smoke; plan §4: and only where the policy writes,
/// as it moves folders and writes HKCU).
fn migrates(options: &Options, policy: WritePolicy) -> bool {
    options.profile.is_none() && !options.smoke_test && policy.writes
}

/// What runs after the arguments and the platform, before the store reads the profile (C# Program.cs:169 `MigrateRenamedIdentifiers`):
/// RenameMigration for the default profile outside a smoke where the policy writes — `%APPDATA%\MightyClaudeNative`, `~\.mightyclaude`
/// with the records and Claude's conversations that name it, and the HKCU key (mighty-core's `rename_migration`) — then `git worktree
/// repair` in each moved worktree on a task of its own (C#'s `Task.Run`), started now and run while the app starts. A failure leaves the
/// old data where it was.
pub fn before_ui(options: &Options, platform: &mighty_platform::Platform, policy: WritePolicy) {
    if !migrates(options, policy) {
        return;
    }
    // The default profile's data folder is `<roaming>\MightyCode`: its parent is C#'s `SpecialFolder.ApplicationData`.
    let Some(app_data) = platform.paths.data_dir().parent().map(Path::to_path_buf) else { return };
    let Some(home) = rename_migration::home() else { return };
    let environment: HashMap<String, String> = platform.environment.current().values.into_iter().collect();
    let claude = mighty_history::history::claude_config(&home, &environment);
    let report = rename_migration::run(&app_data, &home, &claude, registry().as_ref());
    if report.worktrees.is_empty() {
        return;
    }
    let git = mighty_run::git::Git::new(platform.processes.clone(), hooks_folder(platform.paths.data_dir()));
    tauri::async_runtime::spawn(repair_worktrees(git, report.worktrees));
}

/// C# `RenameMigration.RepairWorktreesAsync`: each moved worktree's parent repository pointed at its new place, one git at a time,
/// with delegation's settings (C# `DelegationGit.Run`, its 10 s default).
async fn repair_worktrees(git: mighty_run::git::Git, worktrees: Vec<PathBuf>) {
    for worktree in worktrees {
        let _ = git.run_detailed(&["worktree", "repair"], &worktree, 10.0, None).await;
    }
}

/// The hooks folder git is pointed at while the migration repairs (C# `DelegationGit.NoHooks` before any profile is chosen:
/// `%APPDATA%\MightyCode\git-no-hooks`, the default profile's; `/dev/null` off Windows).
fn hooks_folder(data_dir: &Path) -> PathBuf {
    if cfg!(windows) { data_dir.join("git-no-hooks") } else { PathBuf::from("/dev/null") }
}

/// C# `CurrentUserRegistry`: HKEY_CURRENT_USER on Windows, a registry without keys elsewhere.
fn registry() -> Arc<dyn RegistryStore> {
    #[cfg(windows)]
    {
        Arc::new(CurrentUser(mighty_platform_windows::registry::CurrentUserRegistry::new()))
    }
    #[cfg(not(windows))]
    {
        Arc::new(rename_migration::NoRegistry)
    }
}

/// The Windows registry as RenameMigration reads and writes it.
#[cfg(windows)]
struct CurrentUser(mighty_platform_windows::registry::CurrentUserRegistry);

#[cfg(windows)]
impl RegistryStore for CurrentUser {
    fn key_exists(&self, path: &str) -> bool {
        self.0.key_exists(path)
    }
    fn sub_key_names(&self, path: &str) -> std::io::Result<Vec<String>> {
        self.0.sub_key_names(path)
    }
    fn values(&self, path: &str) -> std::io::Result<Vec<rename_migration::RegistryValue>> {
        let values = self.0.values(path)?.into_iter();
        Ok(values.map(|value| rename_migration::RegistryValue { name: value.name, kind: value.kind, data: value.data }).collect())
    }
    fn create_key(&self, path: &str) -> std::io::Result<()> {
        self.0.create_key(path)
    }
    fn set_value(&self, path: &str, value: &rename_migration::RegistryValue) -> std::io::Result<()> {
        self.0.set_value(path, &value.name, value.kind, &value.data)
    }
    fn delete_tree(&self, path: &str) -> std::io::Result<()> {
        self.0.delete_tree(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Step 0's table in a development build: the flag writes, `--read-only` never does, nothing writes by default; and this build's
    /// policy is the rule over its own stamp and place.
    #[test]
    fn the_flag_writes_and_read_only_never_does() {
        let policy = |allow: bool, read_only: bool| {
            let options = Options { allow_state_write: allow, read_only, ..Options::default() };
            assert_eq!(write_policy(&options), super::policy(build_info::RELEASE && installed(), &options));
            super::policy(false, &options).writes
        };
        assert!(!policy(false, false), "a development build writes nothing unless asked");
        assert!(policy(true, false));
        assert!(!policy(true, true), "--read-only wins");
        assert!(!policy(false, true));
    }

    /// Plan §4's table with review M2's condition: (a release build run from its install folder or not) × `--allow-state-write` ×
    /// `--read-only` × smoke → whether the run writes its profile, `Host::may_change_machine`'s answer over it (writes ∧ not a smoke,
    /// which `may_listen`, `may_reach_relay` and `AutomaticUpdatePolicy` follow), and whether RenameMigration runs (the default profile
    /// only). A development build and a release candidate run from elsewhere answer the same: `installed_release` false.
    #[test]
    fn the_write_policy_table() {
        // (installed release, flag, read-only, smoke) → (writes, may change the machine)
        type Row = ((bool, bool, bool, bool), (bool, bool));
        let rows: [Row; 16] = [
            ((false, false, false, false), (false, false)),
            ((false, false, false, true), (false, false)),
            ((false, false, true, false), (false, false)),
            ((false, false, true, true), (false, false)),
            ((false, true, false, false), (true, true)),
            ((false, true, false, true), (true, false)),
            ((false, true, true, false), (false, false)),
            ((false, true, true, true), (false, false)),
            ((true, false, false, false), (true, true)),
            ((true, false, false, true), (false, false)),
            ((true, false, true, false), (false, false)),
            ((true, false, true, true), (false, false)),
            ((true, true, false, false), (true, true)),
            ((true, true, false, true), (true, false)),
            ((true, true, true, false), (false, false)),
            ((true, true, true, true), (false, false)),
        ];
        for ((installed_release, allow, read_only, smoke), (writes, machine)) in rows {
            let options = Options {
                allow_state_write: allow,
                read_only,
                smoke_test: smoke,
                profile: smoke.then(|| PathBuf::from(r"C:\smoke")),
                ..Options::default()
            };
            let policy = policy(installed_release, &options);
            let row = format!("installed release {installed_release}, flag {allow}, read-only {read_only}, smoke {smoke}");
            assert_eq!(policy.writes, writes, "writes: {row}");
            assert_eq!(policy.installed_release, installed_release);
            // `Host::may_change_machine`'s body over the store's `writes`.
            assert_eq!(policy.writes && !smoke, machine, "may change the machine: {row}");
            assert_eq!(migrates(&options, policy), machine, "RenameMigration: {row}");
            let named = Options { profile: Some(PathBuf::from(r"C:\p")), ..options.clone() };
            assert!(!migrates(&named, policy), "never for --profile: {row}");
        }
    }

    /// Review M2: the test exe runs from the build's target folder, never the install folder, so it is no installed release; the folder
    /// comparison is canonical and a folder that does not exist matches nothing.
    #[test]
    fn only_the_install_folder_counts_as_installed() {
        assert!(!installed(), "the tests never run from the install folder");
        let here = std::env::current_exe().unwrap();
        let folder = here.parent().unwrap();
        assert!(same_folder(folder, &folder.join(".")), "the same folder, written another way");
        assert!(!same_folder(folder, &folder.join("no-such-folder")));
        #[cfg(windows)]
        {
            let install = mighty_platform_windows::install_folder().expect("the known folder");
            assert!(install.ends_with(r"Programs\MightyCode"), "{}", install.display());
            assert!(!same_folder(folder, &install));
        }
    }

    /// Review note 9: a run that writes keeps its log in the profile's `tauri-logs`, and so does a smoke (its own profile, where
    /// smoke.ps1 looks); any other run logs under this machine's local folder, never in the profile; `--log-dir` always wins.
    #[test]
    fn a_run_that_writes_nothing_logs_outside_the_profile() {
        let root = std::env::temp_dir().join("mighty-log-dir");
        let paths =
            mighty_platform::AppPaths::resolve(None, Some(root.join("roam")), &[], Some(root.join("local")), "dev.mightycode.native")
                .unwrap();
        let (on, off) = (WritePolicy { writes: true, installed_release: false }, WritePolicy::default());
        let plain = Options::default();
        assert_eq!(log_dir(&plain, &paths, on), root.join("roam").join("MightyCode").join("tauri-logs"));
        assert_eq!(log_dir(&plain, &paths, off), root.join("local").join("dev.mightycode.native").join("logs").join("default"));
        let smoke = Options { smoke_test: true, ..Options::default() };
        assert_eq!(log_dir(&smoke, &paths, off), paths.logs_dir(), "a smoke's log stays where smoke.ps1 finds it");
        let given = Options { log_dir: Some(root.join("given")), ..Options::default() };
        assert_eq!(log_dir(&given, &paths, on), root.join("given"));
        assert_eq!(log_dir(&given, &paths, off), root.join("given"));
    }

    /// Review M1: C#'s web data is adopted only by a release build on the default profile that writes; L1: only the installed release
    /// on the default profile owns the machine-wide registrations (the test build never does).
    #[test]
    fn csharps_web_data_only_where_the_run_writes() {
        let (on, off) = (WritePolicy { writes: true, installed_release: true }, WritePolicy { writes: false, installed_release: true });
        let plain = Options::default();
        let named = Options { profile: Some(PathBuf::from(r"C:\p")), ..Options::default() };
        assert!(adopts(true, &plain, on));
        assert!(!adopts(true, &plain, off), "--read-only, or a candidate run beside C#: its own folders");
        assert!(!adopts(true, &named, on));
        assert!(!adopts(false, &plain, on), "a development build never");
        assert!(!owns_registrations() || build_info::RELEASE, "an unstamped build never registers");
        assert_eq!(CSHARP_BROWSER_ARGUMENTS, "", "C#'s environment options carry no arguments");
    }

    /// Review M1: the page is made again while its data folder is held (0x8007139F, as WebView2 words it or in decimal), at most for the
    /// waits' ~10 s; any other error comes back at once.
    #[test]
    fn the_page_is_made_again_while_its_folder_is_held() {
        let total: Duration = WEBVIEW_IN_USE_WAITS.iter().sum();
        assert_eq!(total, Duration::from_secs(10));
        let held = "Windows error: The group or resource is not in the correct state (0x8007139F)";
        let (mut tries, mut slept) = (0, Vec::new());
        let made = retry_while_held(
            || {
                tries += 1;
                if tries < 3 { Err(held.to_string()) } else { Ok(tries) }
            },
            |wait| slept.push(wait),
            |_| {},
        );
        assert_eq!(made, Ok(3));
        assert_eq!(slept, WEBVIEW_IN_USE_WAITS[..2]);
        let (mut tries, mut slept) = (0, 0);
        let never = retry_while_held(
            || -> Result<(), String> {
                tries += 1;
                Err("HRESULT -2147019873".into())
            },
            |_| slept += 1,
            |_| {},
        );
        assert!(never.is_err() && tries == 7 && slept == 6, "every wait, then the error ({tries} tries)");
        let mut tries = 0;
        let other = retry_while_held(
            || -> Result<(), String> {
                tries += 1;
                Err("0x80070005 access denied".into())
            },
            |_| panic!("no wait for another error"),
            |_| {},
        );
        assert!(other.is_err() && tries == 1);
    }

    /// The migration's Claude folder name is mighty-history's `ClaudeProjectFolder`, the one Claude names a child's conversations by.
    #[test]
    fn the_migration_names_claude_projects_as_history_does() {
        for path in [r"C:\Users\me\.mightyclaude\worktrees", "/Users/김/.mightycode/worktrees", r"D:\a b\é\x"] {
            assert_eq!(rename_migration::claude_project_folder(path), mighty_history::history::claude_project_folder(path));
        }
    }

    /// The repair's hooks folder is the default profile's own empty folder (C#'s), never the install folder; off Windows git's null.
    #[test]
    fn the_repair_runs_git_without_hooks() {
        let data = Path::new(r"C:\Users\me\AppData\Roaming\MightyCode");
        if cfg!(windows) {
            assert_eq!(hooks_folder(data), data.join("git-no-hooks"));
        } else {
            assert_eq!(hooks_folder(data), Path::new("/dev/null"));
        }
    }
}
