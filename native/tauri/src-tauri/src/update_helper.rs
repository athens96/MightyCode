//! The exe's `--update-helper` mode (C# `WinUI\Program.cs:158,215-271` `RunUpdateHelper`; plan §1.1, W9-B). Owner: W9-B.
//!
//! The 0.4.x app (and later this one) unpacks the package to `updates\<version>\staged` and starts the staged `MightyCode.exe` with the
//! helper's arguments, detached, hidden, in `%TEMP%`, with only `SystemRoot` set. main.rs serves it right after the exe's other modes,
//! before the arguments are parsed, before Tauri, WebView2, the profile, the log or the platform. Here: the plan parsed
//! (`mighty_updates::app_update::plan::try_parse`), the mutex taken through the platform's `UpdateInstaller` (another helper holding it:
//! skipped, exit 0), `replacement::run` with the Windows seams (the start through `ProcessHost::start_detached_or_in_job` with the
//! user's fresh environment, never in a job of this process's), `SyncDisplayVersion` on success with this exe's `major.minor.patch`, one
//! `install.log` line beside the package, the mutex released, exit 0 when replaced or skipped, else 1; all inside `catch_unwind`, the
//! sentences in the OS's language, no Tauri, no profile, no other log. Never elevated, never a window.

use std::io::Write;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use mighty_core::locale::Locale;
use mighty_platform::{DetachedEnvironment, DetachedRequest, ProcessHost, UpdateInstaller};
use mighty_updates::app_update::plan::InstallPlan;
use mighty_updates::app_update::replacement::{self, ReplacementResult, Seams};
use mighty_updates::app_update::{guard, log, plan};

/// The helper's own version, the new version's (C# `typeof(Program).Assembly.GetName().Version.ToString(3)`): what the uninstall
/// entry's `DisplayVersion` becomes.
pub const VERSION: &str =
    concat!(env!("CARGO_PKG_VERSION_MAJOR"), ".", env!("CARGO_PKG_VERSION_MINOR"), ".", env!("CARGO_PKG_VERSION_PATCH"));

/// The helper when `args` are its arguments, with the exit code; None otherwise (the arguments then go to the parser, which refuses
/// `--update-helper`, as C#'s does).
pub fn try_run(args: &[String]) -> Option<i32> {
    let plan = plan::try_parse(args)?;
    let locale = Locale::new(crate::methods::chosen_language("system"));
    let installer = crate::platform::update_installer();
    let processes = crate::platform::process_host();
    let seams = MachineSeams::new(&*installer, &*processes);
    Some(run(&plan, &*installer, &seams, &locale, &|| chrono::Utc::now()))
}

/// C# `RunUpdateHelper(plan)` over `installer` and `seams`: the outcome logged, the exit code answered.
pub fn run(
    plan: &InstallPlan,
    installer: &dyn UpdateInstaller,
    seams: &dyn Seams,
    locale: &Locale,
    now: &dyn Fn() -> chrono::DateTime<chrono::Utc>,
) -> i32 {
    // One helper per install folder: a second one leaves at once, moving and starting nothing (AppUpdateHelperGuard). The mutex is held
    // until the line is written, as C#'s `using` holds it.
    let mutex = installer.helper_mutex(&guard::mutex_name(&plan.install_directory));
    let result = match &mutex {
        Ok(held) => {
            let acquired = held.is_some();
            catch_unwind(AssertUnwindSafe(|| match guard::refusal(acquired) {
                Some(refused) => refused,
                None => {
                    let result = replacement::run(plan, seams, locale);
                    if result.replaced {
                        // C# `InstallerRegistration.SyncDisplayVersion` never throws: a refusal leaves the entry as it was.
                        let _ = installer.sync_display_version(&plan.install_directory, VERSION);
                    }
                    result
                }
            }))
            .unwrap_or_else(|panic| unexpected("Panic", &panic_text(&*panic)))
        }
        // C#'s `new Mutex` would have thrown out of the helper; here the reason is logged like any other unexpected one.
        Err(error) => unexpected("MutexError", &error.to_string()),
    };
    // Appended, so a second helper's line and the first one's outcome are both kept; the outcome never depends on the log.
    let _ = append(&log_path(plan), &log::line(&result, Some(plan), now()));
    drop(mutex);
    exit_code(&result)
}

/// C#'s `catch (Exception error)`: `"<TypeName>: <message>"`, nothing replaced or rolled back.
fn unexpected(kind: &str, message: &str) -> ReplacementResult {
    ReplacementResult { error: Some(format!("{kind}: {message}")), ..ReplacementResult::default() }
}

fn panic_text(panic: &(dyn std::any::Any + Send)) -> String {
    panic
        .downcast_ref::<&str>()
        .map(|text| text.to_string())
        .or_else(|| panic.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "the helper stopped unexpectedly".into())
}

/// `0` when replaced or skipped, else `1` (C# `result.Replaced || result.Skipped ? 0 : 1`).
pub fn exit_code(result: &ReplacementResult) -> i32 {
    if result.replaced || result.skipped { 0 } else { 1 }
}

/// `<folder of --package>\install.log`, or the temporary folder's when the package path has none (C#
/// `Path.Combine(Path.GetDirectoryName(plan.PackagePath) ?? Path.GetTempPath(), "install.log")`).
pub fn log_path(plan: &InstallPlan) -> PathBuf {
    match plan.package_path.parent() {
        Some(folder) => folder.join(log::INSTALL_LOG),
        None => std::env::temp_dir().join(log::INSTALL_LOG),
    }
}

fn append(path: &Path, line: &str) -> std::io::Result<()> {
    std::fs::OpenOptions::new().create(true).append(true).open(path)?.write_all(line.as_bytes())
}

/// The replacement's machine seams (C# `RunUpdateHelper`'s `waitForExit`, `startApp` and `runningAgain`): the platform's process wait,
/// the detached start with the user's fresh environment, and the running-again check polled as C#'s `RunningFromInstallAsync`. The copy,
/// the rename and the retry pause are the replacement's own defaults (C#'s nulls).
pub struct MachineSeams<'a> {
    installer: &'a dyn UpdateInstaller,
    processes: &'a dyn ProcessHost,
    /// How long, and how often, the running-again check looks (C#: 30 s, every 500 ms).
    running_again: (Duration, Duration),
}

impl<'a> MachineSeams<'a> {
    pub fn new(installer: &'a dyn UpdateInstaller, processes: &'a dyn ProcessHost) -> MachineSeams<'a> {
        MachineSeams { installer, processes, running_again: (replacement::RUNNING_AGAIN_TIMEOUT, replacement::RUNNING_AGAIN_POLL) }
    }
}

impl Seams for MachineSeams<'_> {
    /// C# `WaitForExitAsync(plan.AppProcessId, DefaultExitTimeout)`; a wait Windows refuses reads as "still running".
    fn wait_for_exit(&self, plan: &InstallPlan) -> bool {
        let Ok(pid) = u32::try_from(plan.app_process_id) else { return false };
        self.installer.wait_for_exit(pid, replacement::DEFAULT_EXIT_TIMEOUT).unwrap_or(false)
    }

    /// C# Program.cs:236-249: the exe started in its own folder, with the user's own environment when Windows builds it (else this
    /// process's, as C# leaves `ProcessStartInfo.Environment` alone then), never inside a job of the helper's; a job that lets nothing
    /// break away keeps it, as it would keep C#'s.
    fn start_app(&self, executable: &Path) -> std::io::Result<()> {
        let environment = self.installer.fresh_environment().map_or(DetachedEnvironment::Inherit, DetachedEnvironment::Exactly);
        let cwd = executable.parent().map(Path::to_path_buf).unwrap_or_default();
        self.processes
            .start_detached_or_in_job(DetachedRequest {
                program: executable.to_path_buf(),
                args: Vec::new(),
                cwd,
                environment,
                hidden: false,
            })
            .map(|_| ())
            .map_err(|error| std::io::Error::other(error.to_string()))
    }

    /// C# `RunningFromInstallAsync(install, 30 s)`: true while a process other than this one runs from the install folder past the
    /// timeout, false as soon as none does; a process list that cannot be read is an error (the replacement reads it as running).
    fn running_again(&self, plan: &InstallPlan) -> std::io::Result<bool> {
        let (timeout, poll) = self.running_again;
        let deadline = Instant::now() + timeout;
        loop {
            let running =
                self.installer.running_from(&plan.install_directory, true).map_err(|error| std::io::Error::other(error.to_string()))?;
            if !running {
                return Ok(false);
            }
            if Instant::now() >= deadline {
                return Ok(true);
            }
            std::thread::sleep(poll);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use mighty_platform::fake::{FakeProcessHost, FakeUpdateInstaller};

    use super::*;

    fn temp(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "mighty-helper-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ));
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    fn plan_in(root: &Path, relaunch: bool) -> InstallPlan {
        let package = root.join("updates").join("0.5.0").join("MightyCode-windows.zip");
        InstallPlan {
            staged_directory: root.join("updates").join("0.5.0").join("staged"),
            install_directory: root.join("MightyCode"),
            backup_directory: root.join("MightyCode.backup-20261115T030405123"),
            package_path: package,
            sha256: "0".repeat(64),
            size: 1,
            app_process_id: 4242,
            new_executable: root.join("MightyCode").join("MightyCode.exe"),
            previous_executable: root.join("MightyCode.backup-20261115T030405123").join("MightyCode.exe"),
            relaunch,
        }
    }

    /// The helper's version is the exe's own `major.minor.patch` (C# `Version.ToString(3)`), what `DisplayVersion` becomes.
    #[test]
    fn the_version_is_three_numbers() {
        assert_eq!(VERSION.split('.').count(), 3);
        assert!(VERSION.split('.').all(|part| part.parse::<u32>().is_ok()), "{VERSION}");
        assert!(env!("CARGO_PKG_VERSION").starts_with(VERSION));
    }

    /// C#'s exit code and log place: 0 when replaced or skipped, else 1; `install.log` beside the package, the temporary folder's when
    /// the package path has no folder.
    #[test]
    fn the_exit_code_and_the_logs_place_are_csharps() {
        assert_eq!(exit_code(&ReplacementResult { replaced: true, ..ReplacementResult::default() }), 0);
        assert_eq!(exit_code(&ReplacementResult { skipped: true, ..ReplacementResult::default() }), 0);
        assert_eq!(exit_code(&ReplacementResult { rolled_back: true, ..ReplacementResult::default() }), 1);
        assert_eq!(exit_code(&ReplacementResult::default()), 1);
        let root = temp("log");
        let plan = plan_in(&root, true);
        assert_eq!(log_path(&plan), root.join("updates").join("0.5.0").join("install.log"));
        let rootless = InstallPlan { package_path: PathBuf::from(std::path::MAIN_SEPARATOR_STR), ..plan.clone() };
        assert_eq!(log_path(&rootless), std::env::temp_dir().join("install.log"));
        std::fs::remove_dir_all(&root).ok();
    }

    /// C# `RunUpdateHelper`'s guard: another helper holding the install's mutex leaves this one skipped (exit 0), nothing waited for,
    /// moved or started, its reason in `install.log`, appended after the line already there; the mutex is free again afterwards.
    #[test]
    fn a_second_helper_is_skipped_and_logs_why() {
        let root = temp("second");
        let plan = plan_in(&root, true);
        std::fs::create_dir_all(plan.package_path.parent().unwrap()).unwrap();
        std::fs::write(log_path(&plan), "earlier line\n").unwrap();
        let installer = FakeUpdateInstaller::default();
        let processes = FakeProcessHost::default();
        let first = installer.helper_mutex(&guard::mutex_name(&plan.install_directory)).unwrap().expect("the first helper's");
        installer.set_exited(false);
        let seams = MachineSeams::new(&installer, &processes);
        let locale = Locale::new("en");
        let code = run(&plan, &installer, &seams, &locale, &|| chrono::Utc::now());
        assert_eq!(code, 0, "skipped is a success");
        assert!(processes.detached().is_empty() && installer.synced().is_empty(), "nothing started, nothing synced");
        let log = std::fs::read_to_string(log_path(&plan)).unwrap();
        assert!(log.starts_with("earlier line\n"), "appended: {log}");
        assert!(log.contains("skipped=True") && log.contains(guard::ALREADY_RUNNING), "{log}");
        drop(first);
        assert!(installer.held().is_empty());
        std::fs::remove_dir_all(&root).ok();
    }

    /// The relaunch (C# Program.cs:236-249): the new exe started detached in its own folder with exactly the user's fresh environment,
    /// a window allowed, even where this process's job lets nothing break away (it then stays in that job, as C#'s would); without a
    /// fresh environment the helper's own is passed on.
    #[test]
    fn the_app_starts_in_its_folder_with_the_users_environment() {
        let installer = FakeUpdateInstaller::default();
        let processes = FakeProcessHost::default();
        processes.refuse_detached(true);
        let fresh = BTreeMap::from([
            ("USERPROFILE".to_string(), r"C:\Users\me".to_string()),
            ("SystemRoot".to_string(), r"C:\Windows".to_string()),
        ]);
        installer.set_fresh_environment(Some(fresh.clone()));
        let exe = std::env::temp_dir().join("install").join("MightyCode.exe");
        MachineSeams::new(&installer, &processes).start_app(&exe).unwrap();
        installer.set_fresh_environment(None);
        MachineSeams::new(&installer, &processes).start_app(&exe).unwrap();
        let started = processes.detached();
        assert_eq!(started.len(), 2);
        assert_eq!(started[0].program, exe);
        assert_eq!(started[0].cwd, exe.parent().unwrap());
        assert!(started[0].args.is_empty() && !started[0].hidden);
        assert!(started[0].environment == DetachedEnvironment::Exactly(fresh), "exactly the fresh environment");
        assert!(started[1].environment == DetachedEnvironment::Inherit, "the helper's own without one");
    }

    /// C# `RunningFromInstallAsync`: false as soon as nothing runs from the install folder, true when something still does at the
    /// timeout, an error when the process list cannot be read; the wait for the app's exit reads a refused wait as "still running".
    #[test]
    fn the_running_again_check_polls_until_its_timeout() {
        let root = temp("again");
        let plan = plan_in(&root, false);
        let installer = FakeUpdateInstaller::default();
        let processes = FakeProcessHost::default();
        let mut seams = MachineSeams::new(&installer, &processes);
        seams.running_again = (Duration::from_millis(120), Duration::from_millis(20));
        assert!(!seams.running_again(&plan).unwrap());
        installer.set_running(true);
        let started = Instant::now();
        assert!(seams.running_again(&plan).unwrap());
        assert!(started.elapsed() >= Duration::from_millis(100), "it waited for the timeout");
        installer.fail_running(true);
        assert!(seams.running_again(&plan).is_err());
        assert!(seams.wait_for_exit(&plan));
        installer.set_exited(false);
        assert!(!seams.wait_for_exit(&plan));
        std::fs::remove_dir_all(&root).ok();
    }

    /// The uninstall entry's decision is one table on both sides: mighty-platform-windows' (which writes HKCU) and mighty-updates'
    /// `installer_entry` (the shared one, C#'s `VersionToWrite`).
    #[cfg(windows)]
    #[test]
    fn the_uninstall_entrys_decision_is_the_shared_one() {
        const INSTALL: &str = r"C:\Users\me\AppData\Local\Programs\MightyCode";
        let slash = format!(r"{INSTALL}\");
        let upper = INSTALL.to_uppercase();
        let longer = format!("{INSTALL}Old");
        let cases: [(Option<&str>, Option<&str>, &str, &str); 9] = [
            (Some(&slash), Some("0.4.34"), INSTALL, "0.4.35"),
            (Some(&upper), Some("0.4.34"), &slash, "0.4.35"),
            (Some(&slash), Some("0.4.35"), INSTALL, "0.4.35"),
            (Some(&slash), Some("0.4.34"), r"D:\Tools\MightyCode", "0.4.35"),
            (None, Some("0.4.34"), INSTALL, "0.4.35"),
            (Some(&longer), Some("0.4.34"), INSTALL, "0.4.35"),
            (Some(&slash), None, INSTALL, "0.4.35"),
            (Some("  "), Some("0.4.34"), INSTALL, "0.4.35"),
            (Some(&slash), Some("0.4.34"), INSTALL, " "),
        ];
        for (location, shown, folder, version) in cases {
            assert_eq!(
                mighty_platform_windows::update::version_to_write(location, shown, folder, version),
                mighty_updates::app_update::installer_entry::version_to_write(location, shown, Path::new(folder), version),
                "{location:?} {shown:?} {folder} {version}"
            );
        }
    }

    /// A panic inside the replacement becomes C#'s unexpected-exception line: nothing replaced, exit 1.
    #[test]
    fn a_panic_is_logged_as_an_unexpected_failure() {
        struct Panicking;
        impl Seams for Panicking {
            fn wait_for_exit(&self, _plan: &InstallPlan) -> bool {
                panic!("the wait broke")
            }
            fn start_app(&self, _executable: &Path) -> std::io::Result<()> {
                Ok(())
            }
        }
        let root = temp("panic");
        let mut plan = plan_in(&root, true);
        // A whole, valid plan with a real package, so the replacement reaches the wait.
        std::fs::create_dir_all(plan.package_path.parent().unwrap()).unwrap();
        std::fs::create_dir_all(&plan.install_directory).unwrap();
        std::fs::write(plan.install_directory.join("MightyCode.exe"), b"old").unwrap();
        std::fs::write(&plan.package_path, b"package").unwrap();
        plan.size = 7;
        let installer = FakeUpdateInstaller::default();
        let code = run(&plan, &installer, &Panicking, &Locale::new("en"), &|| chrono::Utc::now());
        assert_eq!(code, 1);
        let log = std::fs::read_to_string(log_path(&plan)).unwrap();
        assert!(log.contains("replaced=False rolledBack=False") && log.contains("Panic: the wait broke"), "{log}");
        assert!(installer.held().is_empty(), "the mutex is released after a panic too");
        std::fs::remove_dir_all(&root).ok();
    }
}
