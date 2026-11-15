//! The update helper's Windows seams (W9-B; C# `RunUpdateHelper`'s mutex and start, `AppUpdateReplacement.WaitForExitAsync`,
//! `AnyRunningFrom`, `UserEnvironment.Fresh`, `InstallerRegistration.SyncDisplayVersion`, `RenamedInstall`), against the stand-in child
//! and temporary folders. No test writes HKCU (the uninstall entry's checks run over an entry in memory) or touches the user's own
//! shortcuts (the shortcut checks use folders of their own). The helper itself, the built exe in a bare environment, is
//! `src-tauri/tests/update_helper.rs`'s.
#![cfg(windows)]

mod common;

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use common::{fake, fake_request, pid_from, remove_when_free, temp, until};
use mighty_platform::{DetachedEnvironment, DetachedRequest, PlatformError, ProcessHost, UpdateInstaller};
use mighty_platform_windows::WindowsProcessHost;
use mighty_platform_windows::update::{
    EntryValues, WindowsUpdates, create_shortcut, refresh_shortcuts_in, rename_uninstall_entry_in, running_from, shortcut_target,
    sync_display_version_in, version_to_write,
};

/// The environment entries a `--print-env` (or a started stand-in's `started.env`) wrote.
fn environment_in(file: &Path) -> BTreeMap<String, String> {
    std::fs::read_to_string(file)
        .unwrap()
        .split('\0')
        .filter_map(|entry| entry.split_once('='))
        .filter(|(name, _)| !name.is_empty())
        .map(|(name, value)| (name.to_string(), value.to_string()))
        .collect()
}

fn has(environment: &BTreeMap<String, String>, name: &str) -> bool {
    environment.keys().any(|key| key.eq_ignore_ascii_case(name))
}

/// Whether process `pid` sits in any job (`IsProcessInJob` with no job named).
fn in_any_job(pid: u32) -> Option<bool> {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::JobObjects::IsProcessInJob;
    use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};
    // SAFETY: a query-only open of a process id, closed below.
    unsafe {
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if process.is_null() {
            return None;
        }
        let mut member = 0;
        let ok = IsProcessInJob(process, std::ptr::null_mut(), &mut member) != 0;
        CloseHandle(process);
        ok.then_some(member != 0)
    }
}

/// Plan §W9-B, port D6: the detached start gets exactly the environment it was given (no `NO_COLOR`, nothing of the test's own), runs in
/// the folder given, joins no job of the app's, and answers its id; a relative program or folder, or a NUL, is refused before anything
/// starts. Cargo runs the tests inside a job of its own that lets no child break away: there `start_detached` refuses (the helper's
/// start must not run where that job could end it) and `start_detached_or_in_job` (the relaunched app's) starts the child in it; outside
/// such a job both start a child in no job at all.
#[tokio::test]
async fn a_detached_start_gets_exactly_its_environment_and_joins_no_job() {
    let root = temp("detached");
    let file = root.join("env.txt");
    let host = WindowsProcessHost::new();
    let environment =
        BTreeMap::from([("SystemRoot".to_string(), r"C:\Windows".to_string()), ("MIGHTY_DETACHED_CHECK".to_string(), "a b=c".to_string())]);
    let pid = host
        .start_detached_or_in_job(DetachedRequest {
            program: fake(),
            args: vec!["--print-env".into(), file.to_string_lossy().into_owned()],
            cwd: root.clone(),
            environment: DetachedEnvironment::Exactly(environment.clone()),
            hidden: true,
        })
        .unwrap();
    assert!(pid > 0);
    until(|| file.exists() && std::fs::metadata(&file).is_ok_and(|m| m.len() > 0), 15_000, "the detached child's environment").await;
    let seen = environment_in(&file);
    assert_eq!(seen, environment, "exactly the given variables, nothing inherited, no NO_COLOR");

    // A long child, to look at while it runs.
    let pid_file = root.join("long.pid");
    let long_request = DetachedRequest {
        program: fake(),
        args: vec!["--long-child".into(), pid_file.to_string_lossy().into_owned()],
        cwd: root.clone(),
        environment: DetachedEnvironment::Inherit,
        hidden: true,
    };
    let jobbed = in_any_job(std::process::id()).unwrap();
    match host.start_detached(long_request.clone()) {
        Ok(long) => {
            assert_eq!(pid_from(&pid_file).await, long, "the answer is the started process's id");
            assert_eq!(in_any_job(long), Some(false), "a detached child is in no job, not even the test runner's");
            kill(long);
        }
        Err(PlatformError::Refused(_)) => {
            assert!(jobbed, "only a job that lets no child break away refuses the start");
            let long = host.start_detached_or_in_job(long_request).unwrap();
            assert_eq!(pid_from(&pid_file).await, long, "the answer is the started process's id");
            assert_eq!(in_any_job(long), Some(true), "the relaunch's start stays in the test runner's job");
            kill(long);
        }
        Err(other) => panic!("{other:?}"),
    }

    for bad in [
        DetachedRequest {
            program: PathBuf::from("child.exe"),
            args: vec![],
            cwd: root.clone(),
            environment: DetachedEnvironment::Inherit,
            hidden: true,
        },
        DetachedRequest {
            program: fake(),
            args: vec![],
            cwd: PathBuf::from("relative"),
            environment: DetachedEnvironment::Inherit,
            hidden: true,
        },
        DetachedRequest {
            program: fake(),
            args: vec!["a\0b".into()],
            cwd: root.clone(),
            environment: DetachedEnvironment::Inherit,
            hidden: true,
        },
        DetachedRequest {
            program: fake(),
            args: vec![],
            cwd: root.clone(),
            environment: DetachedEnvironment::Exactly(BTreeMap::from([("A=B".to_string(), "x".to_string())])),
            hidden: true,
        },
    ] {
        assert!(matches!(host.start_detached(bad.clone()), Err(PlatformError::Invalid(_))), "{bad:?}");
    }
    let missing = host.start_detached_or_in_job(DetachedRequest {
        program: root.join("missing.exe"),
        args: vec![],
        cwd: root.clone(),
        environment: DetachedEnvironment::Inherit,
        hidden: true,
    });
    assert!(matches!(missing, Err(PlatformError::Io(_))), "{missing:?}");
    remove_when_free(&root).await;
}

/// C# `UserEnvironment.Fresh` through the port, as the helper hands it to the relaunched app (Program.cs:243-247): the started app has
/// the user's home and PATHEXT although the helper itself had only `SystemRoot`.
#[tokio::test]
async fn the_relaunched_app_gets_the_users_fresh_environment() {
    let root = temp("fresh");
    let fresh = WindowsUpdates.fresh_environment().expect("Windows builds the user's environment");
    assert!(has(&fresh, "USERPROFILE") && has(&fresh, "PATHEXT") && has(&fresh, "SystemRoot"), "{:?}", fresh.keys().collect::<Vec<_>>());
    let install = root.join("install");
    std::fs::create_dir_all(&install).unwrap();
    let app = install.join("MightyCode.exe");
    std::fs::copy(fake(), &app).unwrap();
    WindowsProcessHost::new()
        .start_detached_or_in_job(DetachedRequest {
            program: app.clone(),
            args: vec![],
            cwd: install.clone(),
            environment: DetachedEnvironment::Exactly(fresh.clone()),
            hidden: false,
        })
        .unwrap();
    let started = install.join("started.env");
    until(|| started.exists(), 15_000, "the started app's record").await;
    let text = std::fs::read_to_string(&started).unwrap();
    let own = text.split('\0').next().unwrap();
    assert!(own.eq_ignore_ascii_case(&app.to_string_lossy()), "{own}");
    let seen = environment_in(&started);
    assert!(has(&seen, "USERPROFILE") && has(&seen, "APPDATA") && has(&seen, "LOCALAPPDATA") && !has(&seen, "NO_COLOR"));
    remove_when_free(&root).await;
}

/// C# `RunUpdateHelper`'s guard: one holder of the install's mutex at a time (another thread — Windows hands a mutex to its owner's
/// thread again — gets none), taken again once released, and a holder that ended without releasing it leaves it to the next taker
/// (`AbandonedMutexException` counts as taken).
#[test]
fn the_helper_mutex_has_one_holder_and_an_abandoned_one_is_taken() {
    let name = format!(r"Local\MightyCode-update-helper-test-{}-{:?}", std::process::id(), Instant::now());
    let first = WindowsUpdates.helper_mutex(&name).unwrap();
    assert!(first.is_some(), "the first helper takes it");
    let other = name.clone();
    assert!(std::thread::spawn(move || WindowsUpdates.helper_mutex(&other).unwrap().is_none()).join().unwrap(), "a second helper does not");
    drop(first);
    let other = name.clone();
    // Taken on a thread that ends while holding it: abandoned.
    std::thread::spawn(move || std::mem::forget(WindowsUpdates.helper_mutex(&other).unwrap().expect("free again once released")))
        .join()
        .unwrap();
    let other = name.clone();
    assert!(
        std::thread::spawn(move || WindowsUpdates.helper_mutex(&other).unwrap().is_some()).join().unwrap(),
        "an abandoned mutex is taken"
    );
    assert!(matches!(WindowsUpdates.helper_mutex(""), Err(PlatformError::Invalid(_))));
}

/// C# `WaitForExitAsync`: false while the app runs past the timeout, true once it has exited, and true for a process that is gone.
#[tokio::test]
async fn the_wait_ends_when_the_app_has_exited() {
    let root = temp("wait");
    let host = WindowsProcessHost::new();
    let pid_file = root.join("app.pid");
    let child = host.spawn(fake_request(&["--long-child", &pid_file.to_string_lossy()], &root)).await.unwrap();
    let pid = pid_from(&pid_file).await;
    let started = Instant::now();
    assert!(!WindowsUpdates.wait_for_exit(pid, Duration::from_millis(300)).unwrap(), "still running");
    assert!(started.elapsed() >= Duration::from_millis(250), "it waited the timeout");
    let waiter = std::thread::spawn(move || WindowsUpdates.wait_for_exit(pid, Duration::from_secs(30)).unwrap());
    tokio::time::sleep(Duration::from_millis(200)).await;
    child.kill();
    assert!(waiter.join().unwrap(), "the wait ends at the exit");
    let quick = host.spawn(fake_request(&["--fail-child", "x"], &root)).await.unwrap();
    let gone = quick.id;
    quick.wait().await.unwrap();
    drop(quick);
    assert!(WindowsUpdates.wait_for_exit(gone, Duration::from_secs(5)).unwrap(), "an exited app has exited");
    remove_when_free(&root).await;
}

/// C# `AnyRunningFrom`: a `MightyCode.exe` running from under the folder counts (also when the folder is written in its 8.3 form, as
/// CI's `%TEMP%` is), one running elsewhere, from a folder whose name only starts the same, or this process do not, and nothing once it
/// has exited.
#[tokio::test]
async fn running_from_the_install_folder_sees_the_app_there_only() {
    let root = temp("running");
    let install = root.join("install");
    let elsewhere = root.join("install-elsewhere");
    for folder in [&install, &elsewhere] {
        std::fs::create_dir_all(folder).unwrap();
    }
    let app = install.join("MightyCode.exe");
    std::fs::copy(fake(), &app).unwrap();
    assert!(!running_from(&install, true).unwrap(), "nothing runs from it yet");
    let pid_file = root.join("app.pid");
    let mut request = fake_request(&["--long-child", &pid_file.to_string_lossy()], &root);
    request.program = app.clone();
    let child = WindowsProcessHost::new().spawn(request).await.unwrap();
    pid_from(&pid_file).await;
    assert!(running_from(&install, true).unwrap(), "the app runs from the install folder");
    assert!(
        WindowsUpdates.running_from(&PathBuf::from(format!("{}\\", install.display())), true).unwrap(),
        "a trailing separator is the same folder"
    );
    let upper = PathBuf::from(install.to_string_lossy().to_uppercase());
    assert!(running_from(&upper, true).unwrap(), "folders compare without case");
    if let Some(short) = short_path(&install) {
        assert!(running_from(&short, true).unwrap(), "an 8.3 spelling of the folder is the same folder: {}", short.display());
    }
    assert!(!running_from(&elsewhere, true).unwrap(), "another folder");
    assert!(!running_from(&root.join("inst"), true).unwrap(), "a folder whose name only starts the same is another folder");
    assert!(!running_from(&root.join("missing"), false).unwrap());
    child.kill();
    child.close().await.unwrap();
    until(|| !running_from(&install, true).unwrap(), 10_000, "nothing runs from it once the app has exited").await;
    remove_when_free(&root).await;
}

fn short_path(path: &Path) -> Option<PathBuf> {
    use std::os::windows::ffi::{OsStrExt, OsStringExt};
    use windows_sys::Win32::Storage::FileSystem::GetShortPathNameW;
    let long: Vec<u16> = path.as_os_str().encode_wide().chain(std::iter::once(0)).collect();
    let mut buffer = vec![0u16; 1024];
    // SAFETY: a NUL-terminated path; `buffer` holds its length in units.
    let length = unsafe { GetShortPathNameW(long.as_ptr(), buffer.as_mut_ptr(), buffer.len() as u32) } as usize;
    (length > 0 && length < buffer.len()).then(|| PathBuf::from(std::ffi::OsString::from_wide(&buffer[..length])))
}

fn kill(pid: u32) {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_TERMINATE, TerminateProcess};
    // SAFETY: a terminate-only open of a process this test started, closed below.
    unsafe {
        let process = OpenProcess(PROCESS_TERMINATE, 0, pid);
        if !process.is_null() {
            TerminateProcess(process, 1);
            CloseHandle(process);
        }
    }
}

/// An uninstall entry in memory: the registry's values without the registry.
#[derive(Default)]
struct Entry(HashMap<String, String>, Vec<String>);

impl Entry {
    fn with(values: &[(&str, &str)]) -> Entry {
        Entry(values.iter().map(|(n, v)| (n.to_string(), v.to_string())).collect(), Vec::new())
    }
}

impl EntryValues for Entry {
    fn text(&self, name: &str) -> mighty_platform::Result<Option<String>> {
        Ok(self.0.get(name).cloned())
    }
    fn set_text(&mut self, name: &str, value: &str) -> mighty_platform::Result<()> {
        self.1.push(name.to_string());
        self.0.insert(name.to_string(), value.to_string());
        Ok(())
    }
}

/// C# "app update keeps the setup program's uninstall entry at the app's version" (AppUpdateVerification.cs
/// `InstallerEntryFollowsTheAppsOwnUpdates`): its table for the decision, then the write over an entry in memory (never HKCU).
#[test]
fn the_uninstall_entry_follows_the_apps_own_updates() {
    const INSTALL: &str = r"C:\Users\me\AppData\Local\Programs\MightyCode";
    let lower_install = format!(r"{INSTALL}\");
    assert_eq!(version_to_write(Some(&lower_install), Some("0.4.34"), INSTALL, "0.4.35").as_deref(), Some("0.4.35"));
    assert_eq!(version_to_write(Some(&INSTALL.to_uppercase()), Some("0.4.34"), &lower_install, "0.4.35").as_deref(), Some("0.4.35"));
    assert_eq!(version_to_write(Some(&lower_install), Some("0.4.35"), INSTALL, "0.4.35"), None, "the same version writes nothing");
    assert_eq!(version_to_write(Some(&lower_install), Some("0.4.34"), r"D:\Tools\MightyCode", "0.4.35"), None, "a copy elsewhere");
    assert_eq!(version_to_write(None, Some("0.4.34"), INSTALL, "0.4.35"), None, "no entry writes nothing");
    assert_eq!(
        version_to_write(Some(&format!("{INSTALL}Old")), Some("0.4.34"), INSTALL, "0.4.35"),
        None,
        "a longer name is another folder"
    );
    assert_eq!(version_to_write(Some(&lower_install), None, INSTALL, "0.4.35").as_deref(), Some("0.4.35"), "no version yet takes one");

    let mut entry = Entry::with(&[("InstallLocation", &lower_install), ("DisplayVersion", "0.4.59")]);
    assert!(sync_display_version_in(&mut entry, Path::new(INSTALL), "0.5.0").unwrap());
    assert_eq!(entry.0["DisplayVersion"], "0.5.0");
    assert!(!sync_display_version_in(&mut entry, Path::new(INSTALL), "0.5.0").unwrap(), "written once");
    let mut other = Entry::with(&[("InstallLocation", r"D:\Elsewhere\"), ("DisplayVersion", "0.4.59")]);
    assert!(!sync_display_version_in(&mut other, Path::new(INSTALL), "0.5.0").unwrap() && other.1.is_empty(), "another folder's entry");
    let mut empty = Entry::default();
    assert!(!sync_display_version_in(&mut empty, Path::new(INSTALL), "0.5.0").unwrap() && empty.1.is_empty());

    // RenamedInstall.UpdateUninstallEntry: the icon and name of an entry for this folder that still names the old exe.
    let renamed = format!(r"{INSTALL}\MightyCode.exe");
    let mut old =
        Entry::with(&[("InstallLocation", &lower_install), ("DisplayIcon", r"C:\X\MIGHTYCLAUDE.EXE,0"), ("DisplayName", "Mighty Claude")]);
    assert!(rename_uninstall_entry_in(&mut old, INSTALL, &renamed).unwrap());
    assert_eq!((old.0["DisplayIcon"].as_str(), old.0["DisplayName"].as_str()), (renamed.as_str(), "Mighty Code"));
    assert!(!rename_uninstall_entry_in(&mut old, INSTALL, &renamed).unwrap(), "an icon that names the new exe stays");
    let mut foreign = Entry::with(&[("InstallLocation", r"D:\Other"), ("DisplayIcon", r"D:\Other\MightyClaude.exe")]);
    assert!(!rename_uninstall_entry_in(&mut foreign, INSTALL, &renamed).unwrap() && foreign.1.is_empty());
}

/// C# `RenamedInstall.Refresh`'s shortcuts, in folders of the test's own: one that starts the old exe becomes `Mighty Code.lnk` starting
/// the new one (the old name gone), one that starts something else stays, and nothing changes while the new exe is missing.
#[test]
fn the_renamed_installs_shortcuts_start_the_new_exe() {
    let root = temp("shortcuts");
    let install = root.join("Mighty Claude");
    let (programs, desktop) = (root.join("Programs"), root.join("Desktop"));
    for folder in [&install, &programs, &desktop] {
        std::fs::create_dir_all(folder).unwrap();
    }
    let (old_exe, new_exe) = (install.join("MightyClaude.exe"), install.join("MightyCode.exe"));
    std::fs::write(&old_exe, b"old").unwrap();
    create_shortcut(&programs.join("Mighty Claude.lnk"), &old_exe).unwrap();
    create_shortcut(&desktop.join("Mighty Code.lnk"), &old_exe).unwrap();
    let unrelated = root.join("other.exe");
    std::fs::write(&unrelated, b"x").unwrap();
    create_shortcut(&programs.join("Other.lnk"), &unrelated).unwrap();
    let folders = vec![programs.clone(), desktop.clone()];

    assert!(!refresh_shortcuts_in(&folders, &install), "nothing happens before the new exe is there");
    assert!(programs.join("Mighty Claude.lnk").exists());

    std::fs::write(&new_exe, b"new").unwrap();
    assert!(refresh_shortcuts_in(&folders, &install));
    let same = |a: &str, b: &Path| a.eq_ignore_ascii_case(&b.to_string_lossy());
    assert!(!programs.join("Mighty Claude.lnk").exists(), "the old name is gone");
    assert!(same(&shortcut_target(&programs.join("Mighty Code.lnk")).unwrap(), &new_exe), "Programs starts the new exe");
    assert!(same(&shortcut_target(&desktop.join("Mighty Code.lnk")).unwrap(), &new_exe), "the Desktop's starts the new exe");
    assert!(same(&shortcut_target(&programs.join("Other.lnk")).unwrap(), &unrelated), "another shortcut stays");
    std::fs::remove_dir_all(&root).ok();
}
