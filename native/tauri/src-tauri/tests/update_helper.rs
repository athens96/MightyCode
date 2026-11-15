//! The update helper end to end (W9-B; plan §1.1, R1): the built `MightyCode.exe` started as the 0.4.x app starts it — the helper's
//! arguments, only `SystemRoot=C:\Windows` in its environment, its working folder a temporary one, no window — against a temporary
//! install folder and a temporary staged package, never the real install folder, `%APPDATA%` or HKCU. It proves the swap and the
//! relaunch (with the user's own environment), the rollback when the new app cannot start (the old one started again), the untouched
//! install after a tampered package, the second helper's skip on the same mutex, and the real exe installing itself from the staged
//! folder alone (the install on quit).
//!
//! The "app" the helper starts is this test program copied in as `MightyCode.exe`: started with no argument, as the helper starts the
//! app, the test harness runs every test here, and [`a_started_app_records_itself`] writes the started copy's path and environment to
//! `started.env` beside it while every other test sees it is that copy and returns (C# `Verification.Self` relaunches its own test exe
//! the same way). No real app ever starts.
#![cfg(windows)]
#![allow(
    clippy::disallowed_methods,
    reason = "the test starts the built exe as the 0.4.x app starts the helper: a bare environment, no job of ours"
)]

use std::collections::BTreeMap;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// The built exe under test.
const HELPER: &str = env!("CARGO_BIN_EXE_MightyCode");
/// `CREATE_NO_WINDOW`, as the 0.4.x app's `CreateNoWindow = true`.
const CREATE_NO_WINDOW: u32 = 0x0800_0000;
/// What a started copy writes beside itself.
const STARTED: &str = "started.env";

/// True in the copy the helper started as the app.
fn is_started_copy() -> bool {
    std::env::current_exe().is_ok_and(|exe| exe.file_name().is_some_and(|name| name.eq_ignore_ascii_case("MightyCode.exe")))
}

/// The started app's record: its own path, then its environment, NUL-separated (`.part`, then renamed, so a reader never sees half).
fn record_start(folder: &Path) {
    let own = std::env::current_exe().unwrap();
    let mut entries = vec![own.to_string_lossy().into_owned()];
    entries.extend(std::env::vars_os().map(|(name, value)| format!("{}={}", name.to_string_lossy(), value.to_string_lossy())));
    let part = folder.join(format!("{STARTED}.part"));
    std::fs::write(&part, entries.join("\0")).unwrap();
    std::fs::rename(&part, folder.join(STARTED)).unwrap();
}

/// A record read back: the started exe and its environment.
fn read_start(file: &Path) -> (PathBuf, BTreeMap<String, String>) {
    let text = std::fs::read_to_string(file).unwrap();
    let mut entries = text.split('\0');
    let own = PathBuf::from(entries.next().unwrap());
    let environment = entries
        .filter_map(|entry| entry.split_once('='))
        .filter(|(name, _)| !name.is_empty())
        .map(|(name, value)| (name.to_ascii_uppercase(), value.to_string()))
        .collect();
    (own, environment)
}

/// The app the helper starts: in the started copy, its record beside it; run as a test, the record of this program read back whole.
#[test]
fn a_started_app_records_itself() {
    let own = std::env::current_exe().unwrap();
    if is_started_copy() {
        record_start(own.parent().unwrap());
        return;
    }
    let folder = temp("record");
    record_start(&folder);
    let (started, environment) = read_start(&folder.join(STARTED));
    assert_eq!(started, own);
    assert_eq!(environment.get("PATH"), std::env::var("PATH").ok().as_ref(), "the environment is recorded whole");
    std::fs::remove_dir_all(&folder).ok();
}

fn temp(name: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
    let path = std::env::temp_dir().join(format!("mighty-helper-e2e-{name}-{}-{nanos}", std::process::id()));
    std::fs::create_dir_all(&path).unwrap();
    path
}

/// The folders of one rehearsal: `<root>\MightyCode` (the install), `<root>\updates\0.5.0\{MightyCode-windows.zip, staged}`.
struct Layout {
    root: PathBuf,
    install: PathBuf,
    package: PathBuf,
    staged: PathBuf,
}

impl Layout {
    fn new(name: &str) -> Layout {
        let root = temp(name);
        let version = root.join("updates").join("0.5.0");
        let layout = Layout {
            install: root.join("MightyCode"),
            package: version.join("MightyCode-windows.zip"),
            staged: version.join("staged"),
            root,
        };
        for folder in [&layout.install, &layout.staged] {
            std::fs::create_dir_all(folder).unwrap();
        }
        // The package's bytes are only re-checked (SHA-256 and size), never unpacked, by the helper.
        std::fs::write(&layout.package, b"PK\x05\x06 the package the 0.4.x app downloaded and staged").unwrap();
        layout
    }

    /// The old app: this program as `install\MightyCode.exe`, with a file only the old version has.
    fn old_app(&self) {
        std::fs::copy(std::env::current_exe().unwrap(), self.install.join("MightyCode.exe")).unwrap();
        std::fs::write(self.install.join("old.txt"), b"old").unwrap();
    }

    /// The new app in the staged folder: `exe` as `MightyCode.exe`, with a file and a folder only the new version has.
    fn new_app(&self, exe: &[u8]) {
        std::fs::write(self.staged.join("MightyCode.exe"), exe).unwrap();
        std::fs::write(self.staged.join("new.txt"), b"new").unwrap();
        std::fs::create_dir_all(self.staged.join("claude-mods")).unwrap();
        std::fs::write(self.staged.join("claude-mods").join("mod.json"), b"{}").unwrap();
    }

    fn log(&self) -> String {
        std::fs::read_to_string(self.package.parent().unwrap().join("install.log")).unwrap_or_default()
    }

    /// Folders beside the install: backups and the copy, which a finished helper never leaves.
    fn beside(&self) -> Vec<String> {
        std::fs::read_dir(&self.root)
            .unwrap()
            .filter_map(|entry| entry.ok()?.file_name().into_string().ok())
            .filter(|name| name.starts_with("MightyCode."))
            .collect()
    }

    /// The helper's arguments (C# `HelperArguments`), the package's real SHA-256 and size unless given.
    fn arguments(&self, sha256: Option<&str>, wait_pid: u32, relaunch: bool) -> Vec<String> {
        let digest = sha256.map(str::to_string).unwrap_or_else(|| sha256_of(&self.package));
        let size = std::fs::metadata(&self.package).unwrap().len();
        let backup = self.root.join("MightyCode.backup-20261115T030405123");
        let path = |p: &Path| p.to_string_lossy().into_owned();
        vec![
            "--update-helper".into(),
            "--staged".into(),
            path(&self.staged),
            "--install".into(),
            path(&self.install),
            "--backup".into(),
            path(&backup),
            "--package".into(),
            path(&self.package),
            "--sha256".into(),
            digest,
            "--size".into(),
            size.to_string(),
            "--wait-pid".into(),
            wait_pid.to_string(),
            "--relaunch".into(),
            if relaunch { "1" } else { "0" }.into(),
        ]
    }

    fn remove(&self) {
        for _ in 0..40 {
            if !self.root.exists() || std::fs::remove_dir_all(&self.root).is_ok() {
                return;
            }
            std::thread::sleep(Duration::from_millis(250));
        }
    }
}

fn sha256_of(path: &Path) -> String {
    mighty_composer::images::sha256(&std::fs::read(path).unwrap())
}

/// Starts `helper` as the 0.4.x app starts it (MainWindow.AppUpdate.cs:65-84): its arguments, an environment of `SystemRoot=C:\Windows`
/// alone, the temporary folder as its working folder, no window; waits for its exit code (2 minutes at most).
fn run_helper(helper: &Path, arguments: &[String], cwd: &Path) -> i32 {
    let mut child = Command::new(helper)
        .args(arguments)
        .env_clear()
        .env("SystemRoot", r"C:\Windows")
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .unwrap();
    let end = Instant::now() + Duration::from_secs(120);
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            return status.code().unwrap();
        }
        if Instant::now() > end {
            child.kill().ok();
            panic!("the helper did not end within 2 minutes");
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// The id of a process that has already exited (the app that started the helper is gone).
fn exited_pid() -> u32 {
    let mut child =
        Command::new(std::env::current_exe().unwrap()).arg("--list").stdout(Stdio::null()).stderr(Stdio::null()).spawn().unwrap();
    child.wait().unwrap();
    child.id()
}

/// Waits for `file` to appear (30 s at most).
fn appears(file: &Path) -> bool {
    let end = Instant::now() + Duration::from_secs(30);
    while Instant::now() < end {
        if file.exists() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    false
}

fn same_path(a: &Path, b: &Path) -> bool {
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

/// C# "app update helper copies beside the install and swaps by renaming" with the real exe: the helper waits for the app to quit (a
/// process still running for a moment), re-checks the package, swaps the folders, starts the new app with the user's own environment
/// (the helper itself had only `SystemRoot`), removes the backup once it started, logs C#'s line and exits 0.
#[test]
fn the_helper_swaps_relaunches_and_logs() {
    if is_started_copy() {
        return;
    }
    let layout = Layout::new("swap");
    layout.old_app();
    layout.new_app(&std::fs::read(std::env::current_exe().unwrap()).unwrap());
    // The app that started the helper ends two seconds later (a loopback ping standing in for it): the helper must wait for it.
    let ping = PathBuf::from(std::env::var_os("SystemRoot").unwrap()).join("System32").join("PING.EXE");
    let mut app = Command::new(ping).args(["-n", "3", "127.0.0.1"]).stdout(Stdio::null()).stderr(Stdio::null()).spawn().unwrap();
    let code = run_helper(Path::new(HELPER), &layout.arguments(None, app.id(), true), &layout.root);
    let app_ended = app.try_wait().unwrap().is_some();
    app.wait().unwrap();
    assert!(app_ended, "the helper ended only after the app had exited");
    let log = layout.log();
    assert_eq!(code, 0, "{log}");
    assert!(layout.install.join("new.txt").exists() && !layout.install.join("old.txt").exists(), "the install is the new version");
    assert!(layout.install.join("claude-mods").join("mod.json").exists(), "the whole staged tree");
    assert!(layout.staged.join("MightyCode.exe").exists(), "the staged folder is copied, not moved");
    assert!(appears(&layout.install.join(STARTED)), "the new app was started");
    let (started, environment) = read_start(&layout.install.join(STARTED));
    assert!(same_path(&started, &layout.install.join("MightyCode.exe")), "{}", started.display());
    assert!(environment.contains_key("USERPROFILE") && environment.contains_key("LOCALAPPDATA"), "the user's own environment");
    assert!(!environment.contains_key("NO_COLOR"), "nothing the app's children get");
    assert!(layout.beside().is_empty(), "no backup or copy left: {:?}", layout.beside());
    let line = log.lines().last().unwrap_or_default();
    assert!(
        line.contains(" replaced=True rolledBack=False relaunch=True skipped=False started=True backupRemoved=True "),
        "C#'s line: {line}"
    );
    layout.remove();
}

/// C# "app update helper rolls back after a copy that fails half way" at the start: a new exe Windows cannot start puts the backup back,
/// starts the old app again and exits 1 (`rolledBack=True`).
#[test]
fn a_new_app_that_cannot_start_is_rolled_back_and_the_old_one_started() {
    if is_started_copy() {
        return;
    }
    let layout = Layout::new("rollback");
    layout.old_app();
    layout.new_app(b"this is not an executable");
    let code = run_helper(Path::new(HELPER), &layout.arguments(None, exited_pid(), true), &layout.root);
    let log = layout.log();
    assert_eq!(code, 1, "{log}");
    assert!(layout.install.join("old.txt").exists() && !layout.install.join("new.txt").exists(), "the old version is back");
    assert!(appears(&layout.install.join(STARTED)), "the old app was started again");
    let (started, _) = read_start(&layout.install.join(STARTED));
    assert!(same_path(&started, &layout.install.join("MightyCode.exe")));
    assert!(layout.beside().is_empty(), "no backup or copy left: {:?}", layout.beside());
    let line = log.lines().last().unwrap_or_default();
    assert!(line.contains(" replaced=False rolledBack=True relaunch=True skipped=False started=True backupRemoved=False "), "{line}");
    layout.remove();
}

/// C# "app update helper verifies the package again before the install is moved aside": a package that changed since it was checked
/// leaves the install as it was, starts nothing (C#: the re-check returns before any start) and exits 1.
#[test]
fn a_tampered_package_leaves_the_install_untouched() {
    if is_started_copy() {
        return;
    }
    let layout = Layout::new("tampered");
    layout.old_app();
    layout.new_app(&std::fs::read(std::env::current_exe().unwrap()).unwrap());
    let code = run_helper(Path::new(HELPER), &layout.arguments(Some(&"ab".repeat(32)), exited_pid(), true), &layout.root);
    let log = layout.log();
    assert_eq!(code, 1, "{log}");
    assert!(layout.install.join("old.txt").exists() && !layout.install.join("new.txt").exists(), "the install is untouched");
    std::thread::sleep(Duration::from_secs(2));
    assert!(!layout.install.join(STARTED).exists(), "nothing was started");
    assert!(layout.beside().is_empty(), "{:?}", layout.beside());
    let line = log.lines().last().unwrap_or_default();
    assert!(line.contains(" replaced=False rolledBack=False relaunch=True skipped=False started=False backupRemoved=False "), "{line}");
    layout.remove();
}

/// C#'s guard across processes: while another holds the install's mutex (`Local\MightyCode-update-helper-<32 hex>`, the name
/// mighty-updates' `guard::mutex_name` gives), the helper moves and starts nothing, logs why and exits 0.
#[test]
fn a_second_helper_on_the_same_install_is_skipped() {
    if is_started_copy() {
        return;
    }
    use mighty_platform::UpdateInstaller;
    let layout = Layout::new("second");
    layout.old_app();
    layout.new_app(&std::fs::read(std::env::current_exe().unwrap()).unwrap());
    let name = mighty_updates::app_update::guard::mutex_name(&layout.install);
    let held = mighty_platform_windows::WindowsUpdates.helper_mutex(&name).unwrap().expect("the first helper's mutex");
    let code = run_helper(Path::new(HELPER), &layout.arguments(None, exited_pid(), true), &layout.root);
    drop(held);
    let log = layout.log();
    assert_eq!(code, 0, "{log}");
    assert!(layout.install.join("old.txt").exists() && layout.staged.join("new.txt").exists(), "nothing moved");
    let line = log.lines().last().unwrap_or_default();
    assert!(line.contains(" replaced=False rolledBack=False relaunch=True skipped=True started=False backupRemoved=False "), "{line}");
    assert!(line.ends_with(mighty_updates::app_update::guard::ALREADY_RUNNING), "{line}");
    layout.remove();
}

/// The 0.4.x → 0.5 hand-off itself, the install on quit: the real exe, alone in the staged folder, runs as the helper from there in a
/// bare environment and puts itself in the install folder (`--relaunch 0`: nothing started, the backup removed once the new folder is in).
#[test]
fn the_real_exe_installs_itself_from_the_staged_folder() {
    if is_started_copy() {
        return;
    }
    let layout = Layout::new("on-quit");
    layout.old_app();
    let built = std::fs::read(HELPER).unwrap();
    layout.new_app(&built);
    let helper = layout.staged.join("MightyCode.exe");
    let code = run_helper(&helper, &layout.arguments(None, exited_pid(), false), &layout.root);
    let log = layout.log();
    assert_eq!(code, 0, "{log}");
    assert_eq!(std::fs::read(layout.install.join("MightyCode.exe")).unwrap(), built, "the install holds the new exe");
    assert!(!layout.install.join("old.txt").exists());
    std::thread::sleep(Duration::from_secs(1));
    assert!(!layout.install.join(STARTED).exists(), "nothing was started");
    assert!(layout.beside().is_empty(), "{:?}", layout.beside());
    let line = log.lines().last().unwrap_or_default();
    assert!(line.contains(" replaced=True rolledBack=False relaunch=False skipped=False started=False backupRemoved=True "), "{line}");
    layout.remove();
}
