//! M-A: the login shell's environment for the CLIs. Swift `MightyCore/CLIEnvironmentResolver.swift`; contract
//! `mighty_platform::environment` (`ShellEnvironment`: never fails, 4 s, reused 10 s).
//!
//! `/bin/zsh -ilc "printf '\0MIGHTY_ENV_<nonce>\0'; /usr/bin/env -0; printf '\0MIGHTY_END_<nonce>\0'"` with the app's environment
//! exactly (Swift's `ProviderService.runtimeEnvironment`), in the user's home, within 4 s and 1 MB. **A deliberate difference from
//! Swift**, whose resolver captured in the workspace's folder: one capture in the home serves every workspace, as on Windows (C#'s
//! rule) — a login shell started in a cloned repository would run that repository's shell hooks (a `.env` an oh-my-zsh plugin sources,
//! direnv), and one workspace's capture would become every terminal's `current()`. Requests of one moment share a capture (a 50 ms
//! coalesce before any credential is read); a result is reused 10 s; a forced resolve joins only a capture that has not started yet,
//! and a capture superseded or invalidated is ended with its shell. Only the frame's markers are in the shell's arguments; startup
//! chatter outside the frame is not parsed, and neither stream is logged.
//! A row without `=`, a name holding one, a repeated name or a missing or empty PATH refuses the capture; a refused, failed or late
//! capture falls back to the app's environment. The login PATH ends with the installers' folders (`~/.local/bin`,
//! `~/.npm-global/bin`, `~/.bun/bin`, `/opt/homebrew/bin`, `/usr/local/bin`) for the real environment only.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use mighty_platform::process::ChildReader;
use mighty_platform::{BoxFuture, CliEnvironment, EnvironmentSource, ProcessHost, ShellEnvironment, SpawnRequest};
use tokio::io::AsyncReadExt;
use tokio::sync::watch;

use crate::process::{CloseOnDrop, lock};

/// The capture's bound (Swift's `timeout: 4`).
pub const TIMEOUT: Duration = Duration::from_secs(4);
/// How long a capture is reused (Swift's `cacheTTL: 10`).
pub const CACHE_FOR: Duration = Duration::from_secs(10);
/// The most a capture may print.
pub const MAXIMUM_OUTPUT: usize = 1_048_576;
/// Requests of one refresh share a capture: it waits this long before it starts.
pub const COALESCE: Duration = Duration::from_millis(50);

/// The Mac's `ShellEnvironment` over the Mac process host.
pub fn shell_environment() -> Arc<dyn ShellEnvironment> {
    Arc::new(MacShellEnvironment::new(crate::process::process_host(), EnvironmentOptions::default()))
}

/// Swift `CLIEnvironmentResolver.wellKnownFolders`: where the official installers put the CLIs, appended to the login PATH.
pub fn well_known_folders(home: &Path) -> Vec<String> {
    let home = home.to_string_lossy();
    vec![
        format!("{home}/.local/bin"),
        format!("{home}/.npm-global/bin"),
        format!("{home}/.bun/bin"),
        "/opt/homebrew/bin".into(),
        "/usr/local/bin".into(),
    ]
}

/// Swift `CLIEnvironmentResolver.appending`: `path` with each of `folders` it does not hold yet added at the end.
pub fn appending(folders: &[String], path: &str) -> String {
    let mut entries: Vec<String> = path.split(':').filter(|entry| !entry.is_empty()).map(String::from).collect();
    for folder in folders {
        if !entries.contains(folder) {
            entries.push(folder.clone());
        }
    }
    entries.join(":")
}

/// Swift `ProviderService.runtimeEnvironment`: this process's environment with the installers' folders and the system's after its
/// PATH, each once. A variable whose name or value is not UTF-8 is left out (`std::env::vars` would panic on it).
pub fn runtime_environment(home: &Path) -> BTreeMap<String, String> {
    let mut environment: BTreeMap<String, String> =
        std::env::vars_os().filter_map(|(name, value)| Some((name.into_string().ok()?, value.into_string().ok()?))).collect();
    let home = home.to_string_lossy();
    let extra = [
        format!("{home}/.local/bin"),
        format!("{home}/.npm-global/bin"),
        "/opt/homebrew/bin".into(),
        "/usr/local/bin".into(),
        "/usr/bin".into(),
        "/bin".into(),
    ];
    let mut seen: Vec<String> = Vec::new();
    for entry in environment.get("PATH").map(String::as_str).unwrap_or("").split(':').map(String::from).chain(extra) {
        if !entry.is_empty() && !seen.contains(&entry) {
            seen.push(entry);
        }
    }
    environment.insert("PATH".into(), seen.join(":"));
    environment
}

/// Swift `CLIEnvironmentResolver.parse`: the `NAME=value` rows between the two NUL-framed markers, or None for a capture that is
/// unframed, malformed (a row without `=`, an empty name or one holding `=`, a repeated name, text that is not UTF-8) or without a
/// PATH.
pub fn parse(data: &[u8], start: &str, end: &str) -> Option<BTreeMap<String, String>> {
    let framed = |marker: &str| [&[0u8][..], marker.as_bytes(), &[0u8]].concat();
    let (opening, closing) = (framed(start), framed(end));
    let from = find(data, &opening, 0)? + opening.len();
    let to = find(data, &closing, from)?;
    let mut environment = BTreeMap::new();
    for row in data[from..to].split(|byte| *byte == 0).filter(|row| !row.is_empty()) {
        let separator = row.iter().position(|byte| *byte == b'=')?;
        if separator == 0 {
            return None;
        }
        let name = std::str::from_utf8(&row[..separator]).ok()?;
        let value = std::str::from_utf8(&row[separator + 1..]).ok()?;
        if environment.insert(name.to_string(), value.to_string()).is_some() {
            return None;
        }
    }
    // A truncated or empty capture must not silently strip the runtime PATH.
    environment.get("PATH").filter(|path| !path.is_empty())?;
    Some(environment)
}

fn find(haystack: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    if needle.is_empty() || haystack.len() < from + needle.len() {
        return None;
    }
    haystack[from..].windows(needle.len()).position(|window| window == needle).map(|at| at + from)
}

/// What the resolver captures with (Swift `CLIEnvironmentResolver.init`'s arguments).
#[derive(Clone)]
pub struct EnvironmentOptions {
    /// The capture's environment and the fallback; None: this process's ([`runtime_environment`]).
    pub base: Option<BTreeMap<String, String>>,
    pub shell: PathBuf,
    /// The folder every capture runs in.
    pub home: PathBuf,
    pub timeout: Duration,
    pub cache_for: Duration,
    /// How long requests are gathered before a capture starts ([`COALESCE`]).
    pub coalesce: Duration,
    /// Folders the login PATH ends with; None: the installers' ([`well_known_folders`]) for the real environment, none for a given one.
    pub appended: Option<Vec<String>>,
}

impl Default for EnvironmentOptions {
    fn default() -> EnvironmentOptions {
        EnvironmentOptions {
            base: None,
            shell: PathBuf::from("/bin/zsh"),
            home: dirs::home_dir().unwrap_or_else(|| PathBuf::from("/")),
            timeout: TIMEOUT,
            cache_for: CACHE_FOR,
            coalesce: COALESCE,
            appended: None,
        }
    }
}

/// Reads a pipe to its end, None past `limit` bytes.
async fn read_capped(reader: Option<ChildReader>, limit: usize) -> Option<Vec<u8>> {
    let mut reader = reader?;
    let mut out = Vec::new();
    let mut buffer = [0u8; 8192];
    loop {
        let read = reader.read(&mut buffer).await.ok()?;
        if read == 0 {
            return Some(out);
        }
        if out.len() + read > limit {
            return None;
        }
        out.extend_from_slice(&buffer[..read]);
    }
}

/// The capture (Swift's `Entry`).
struct Entry {
    id: u64,
    created: Instant,
    result: watch::Receiver<Option<CliEnvironment>>,
    /// Nobody has had its result yet.
    in_flight: bool,
    /// The capture began (after the coalesce): a forced resolve no longer joins it.
    started: Arc<AtomicBool>,
    task: tokio::task::AbortHandle,
}

struct Inner {
    processes: Arc<dyn ProcessHost>,
    options: EnvironmentOptions,
    appended: Vec<String>,
    entry: Mutex<Option<Entry>>,
    last: Mutex<Option<CliEnvironment>>,
    next: AtomicU64,
}

/// Swift `CLIEnvironmentResolver` as the platform's [`ShellEnvironment`].
pub struct MacShellEnvironment {
    inner: Arc<Inner>,
}

impl MacShellEnvironment {
    pub fn new(processes: Arc<dyn ProcessHost>, options: EnvironmentOptions) -> MacShellEnvironment {
        let appended =
            options.appended.clone().unwrap_or_else(|| if options.base.is_none() { well_known_folders(&options.home) } else { vec![] });
        MacShellEnvironment {
            inner: Arc::new(Inner {
                processes,
                options,
                appended,
                entry: Mutex::new(None),
                last: Mutex::new(None),
                next: AtomicU64::new(1),
            }),
        }
    }
}

impl Inner {
    fn base(&self) -> BTreeMap<String, String> {
        self.options.base.clone().unwrap_or_else(|| runtime_environment(&self.options.home))
    }

    fn fallback(&self) -> CliEnvironment {
        CliEnvironment { values: self.base(), source: EnvironmentSource::ProcessFallback }
    }

    fn is_current(&self, id: u64) -> bool {
        lock(&self.entry).as_ref().is_some_and(|entry| entry.id == id)
    }

    /// The capture itself, in the home: None when the shell failed, was late, exited non-zero or printed no well-framed environment.
    /// The shell is ended and closed however the capture ends — a capture dropped half-way (superseded, invalidated) too.
    async fn capture(&self) -> Option<CliEnvironment> {
        let base = self.base();
        let nonce = nonce();
        let (start, end) = (format!("MIGHTY_ENV_{nonce}"), format!("MIGHTY_END_{nonce}"));
        let command = format!("printf '\\000{start}\\000'; /usr/bin/env -0; printf '\\000{end}\\000'");
        let removed: Vec<String> =
            std::env::vars_os().filter_map(|(name, _)| name.into_string().ok()).filter(|name| !base.contains_key(name)).collect();
        let request = SpawnRequest {
            program: self.options.shell.clone(),
            args: vec!["-ilc".into(), command],
            cwd: self.options.home.clone(),
            env: base,
            env_remove: removed,
            shell_command: None,
        };
        let mut child = self.processes.spawn(request).await.ok()?;
        let guard = CloseOnDrop(Some(child.control()));
        let control = child.control();
        drop(child.stdin.take());
        let (stdout, stderr) = (child.stdout.take(), child.stderr.take());
        let work = async {
            let (output, _errors, code) =
                tokio::join!(read_capped(stdout, MAXIMUM_OUTPUT), read_capped(stderr, MAXIMUM_OUTPUT), control.wait());
            Some((output?, code.ok()?))
        };
        let outcome = tokio::time::timeout(self.options.timeout, work).await.ok().flatten();
        if outcome.is_none() {
            control.kill();
        }
        guard.disarm().close().await.ok()?;
        let (output, code) = outcome?;
        if code != 0 {
            return None;
        }
        let mut values = parse(&output, &start, &end)?;
        let path = values.get("PATH").cloned().unwrap_or_default();
        values.insert("PATH".into(), appending(&self.appended, &path));
        Some(CliEnvironment { values, source: EnvironmentSource::Profile })
    }

    /// Swift `CLIEnvironmentResolver.resolve`, with the one capture in the home.
    fn resolve(self: &Arc<Self>, force: bool) -> BoxFuture<'static, CliEnvironment> {
        let this = self.clone();
        Box::pin(async move {
            let (id, mut result) = {
                let mut entry = lock(&this.entry);
                let reuse = entry.as_ref().is_some_and(|cached| {
                    if force {
                        cached.in_flight && !cached.started.load(Ordering::SeqCst)
                    } else {
                        cached.in_flight || cached.created.elapsed() < this.options.cache_for
                    }
                });
                if !reuse {
                    if let Some(old) = entry.take() {
                        old.task.abort();
                    }
                    let id = this.next.fetch_add(1, Ordering::SeqCst);
                    let started = Arc::new(AtomicBool::new(false));
                    let (sender, result) = watch::channel(None);
                    let (worker, flag) = (this.clone(), started.clone());
                    let task = tokio::spawn(async move {
                        // Requests from one refresh share the capture; once it starts, a forced request supersedes it, so a changed
                        // sign-in never joins stale work.
                        tokio::time::sleep(worker.options.coalesce).await;
                        let outcome = if worker.is_current(id) {
                            flag.store(true, Ordering::SeqCst);
                            worker.capture().await
                        } else {
                            None
                        };
                        sender.send_replace(Some(outcome.unwrap_or_else(|| worker.fallback())));
                    });
                    *entry = Some(Entry { id, created: Instant::now(), result, in_flight: true, started, task: task.abort_handle() });
                }
                let entry = entry.as_ref().expect("just made or found");
                (entry.id, entry.result.clone())
            };
            let value = match result.wait_for(Option::is_some).await {
                Ok(value) => value.clone().unwrap_or_else(|| this.fallback()),
                Err(_) => this.fallback(),
            };
            let current = match lock(&this.entry).as_mut() {
                Some(entry) if entry.id == id => {
                    entry.in_flight = false;
                    true
                }
                _ => false,
            };
            if !current {
                // Superseded (a forced refresh replaced it, or it was invalidated): the current capture answers.
                return this.resolve(false).await;
            }
            *lock(&this.last) = Some(value.clone());
            value
        })
    }
}

/// A frame marker's random part: the time, the process and a count (only frame markers are in the shell's arguments).
fn nonce() -> String {
    static COUNT: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    format!("{nanos:x}{:x}{:x}", std::process::id(), COUNT.fetch_add(1, Ordering::Relaxed))
}

impl ShellEnvironment for MacShellEnvironment {
    /// The one capture in the home, whatever `workspace` (see the module: the Mac never captures in a project's folder).
    fn resolve(&self, _workspace: Option<&Path>, force: bool) -> BoxFuture<'_, CliEnvironment> {
        self.inner.resolve(force)
    }

    fn current(&self) -> CliEnvironment {
        lock(&self.inner.last).clone().unwrap_or_else(|| self.inner.fallback())
    }

    /// Forgets the capture (there is one, the home's, whatever `workspace`); one still running is ended with its shell.
    fn invalidate(&self, _workspace: Option<&Path>) {
        if let Some(old) = lock(&self.inner.entry).take() {
            old.task.abort();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Swift `CLIEnvironmentTests.swift:155 "theLoginPathEndsWithTheInstallersFoldersOnlyForTheRealEnvironment"` (its pure half; the
    /// capture's half is in tests/environment.rs).
    #[test]
    fn the_installers_folders_are_appended_once() {
        assert_eq!(appending(&["/opt/homebrew/bin".into(), "/a".into()], "/a:/usr/bin"), "/a:/usr/bin:/opt/homebrew/bin");
        assert_eq!(appending(&["/x".into()], ""), "/x");
        assert_eq!(
            well_known_folders(Path::new("/Users/fixture")),
            [
                "/Users/fixture/.local/bin",
                "/Users/fixture/.npm-global/bin",
                "/Users/fixture/.bun/bin",
                "/opt/homebrew/bin",
                "/usr/local/bin"
            ]
        );
    }

    #[test]
    fn a_frame_is_read_only_whole_and_once() {
        let framed = "noise\0start\0PATH=/bin\0MULTI=line1\nline2=끝\0\0end\0trailing".as_bytes();
        let parsed = parse(framed, "start", "end").unwrap();
        assert_eq!(parsed.get("MULTI").map(String::as_str), Some("line1\nline2=끝"));
        assert!(parse(b"\0start\0PATH=/bin\0PATH=/usr/bin\0\0end\0", "start", "end").is_none(), "a repeated name");
        assert!(parse(b"\0start\0PATH=/bin", "start", "end").is_none(), "no closing marker");
        assert!(parse(b"\0start\0HOME=/x\0\0end\0", "start", "end").is_none(), "no PATH");
        assert!(parse(b"\0start\0PATH=\0\0end\0", "start", "end").is_none(), "an empty PATH");
        assert!(parse(b"\0start\0=x\0PATH=/bin\0\0end\0", "start", "end").is_none(), "an empty name");
        assert!(parse(b"\0start\0junk\0PATH=/bin\0\0end\0", "start", "end").is_none(), "a row without =");
    }
}
