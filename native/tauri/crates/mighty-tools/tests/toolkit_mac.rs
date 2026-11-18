//! Swift `ToolkitProcessTests.swift`: the toolkit's commands are found on the login shell's PATH they are given (not the app's own,
//! and with no PATHEXT), run with the given environment, and a command that prints more than a pipe holds, one past its time and one
//! leaving a background child holding the pipe all end. Over the Mac process host: the toolkit executor (`ProcessToolkitExecutor`)
//! with the Mac's lookup (`find_executable`, what platform.rs' `find_on_path` gives the executor there), and the run it makes
//! (`mighty_tools::process::run`) where a check needs a shorter time than a step's 5 minutes. Mac only: the fixtures are `/bin/sh`
//! scripts, as Swift's.
#![cfg(target_os = "macos")]

mod common;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use mighty_platform::fake::FakeShellEnvironment;
use mighty_platform_macos::process::find_executable;
use mighty_tools::toolkit::runner::{ProcessToolkitExecutor, ToolkitExecutor};

fn script(folder: &Path, name: &str, body: &str) -> PathBuf {
    use std::os::unix::fs::PermissionsExt as _;
    let path = folder.join(name);
    std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path
}

fn environment(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs.iter().map(|(name, value)| (name.to_string(), value.to_string())).collect()
}

fn executor(environment: BTreeMap<String, String>, folder: &Path) -> ProcessToolkitExecutor {
    ProcessToolkitExecutor {
        processes: common::processes(),
        environment: Arc::new(FakeShellEnvironment::new(environment)),
        resolve: Arc::new(|name: &str, environment: &BTreeMap<String, String>| find_executable(name, environment)),
        working_directory: folder.to_path_buf(),
    }
}

fn argv(parts: &[&str]) -> Vec<String> {
    parts.iter().map(|part| part.to_string()).collect()
}

/// Runs `program` found on `path` with a time limit of its own (the run a toolkit step makes).
async fn run(program: &str, path: &str, folder: &Path, timeout: Duration) -> (mighty_tools::process::Ran, Duration) {
    let found = find_executable(program, &environment(&[("PATH", path)])).expect("the program is on the PATH");
    let host = common::processes();
    let request = host.cli_request(Path::new(&found), &[], folder, &environment(&[("PATH", path)])).unwrap();
    let started = Instant::now();
    let ran = mighty_tools::process::run(host.as_ref(), request, None, timeout, 1 << 20).await.unwrap();
    (ran, started.elapsed())
}

/// Swift `ToolkitProcessTests.swift:20 "executablesAreFoundOnTheGivenPathOnly"`.
#[tokio::test(flavor = "multi_thread")]
async fn executables_are_found_on_the_given_path_only() {
    let root = common::temp("toolkit-path");
    let name = format!("mighty-toolkit-fixture-{}", std::process::id());
    let fixture = script(&root, &name, "echo found");
    let path = format!("/usr/bin:{}", root.display());
    assert_eq!(find_executable(&name, &environment(&[("PATH", &path)])), Some(fixture.to_string_lossy().into_owned()));
    // The app's own PATH does not hold the fixture folder, and an empty PATH finds nothing.
    assert_eq!(find_executable(&name, &std::env::vars().collect()), None);
    assert_eq!(find_executable(&name, &BTreeMap::new()), None);
    // A relative path is never resolved against the working folder.
    assert_eq!(find_executable(&format!("./{name}"), &environment(&[("PATH", &root.to_string_lossy())])), None);
    let output = executor(environment(&[("PATH", &root.to_string_lossy())]), &root).run(&argv(&[&name])).await;
    assert_eq!(output.exit_code, 0, "{output:?}");
    common::remove(&root);
}

/// Swift `ToolkitProcessTests.swift:34 "theCommandRunsWithTheGivenEnvironment"`.
#[tokio::test(flavor = "multi_thread")]
async fn the_command_runs_with_the_given_environment() {
    let root = common::temp("toolkit-env");
    script(&root, "show-env", "printf '%s' \"$MIGHTY_FIXTURE\"; [ \"$MIGHTY_FIXTURE\" = login-shell ] || exit 9");
    let path = format!("{}:/usr/bin:/bin", root.display());
    let output = executor(environment(&[("PATH", &path), ("MIGHTY_FIXTURE", "login-shell")]), &root).run(&argv(&["show-env"])).await;
    assert_eq!(output.exit_code, 0, "{output:?}");
    let (ran, _) = run("show-env", &path, &root, Duration::from_secs(10)).await;
    assert_eq!(ran.exit_code, 9, "without the variable the fixture says so: the environment is the given one");
    common::remove(&root);
}

/// Swift `ToolkitProcessTests.swift:41 "largeOutputIsDrainedWhileTheCommandRuns"`: 2 MB on stdout and stderr together, far past a pipe's
/// 64 KB, and the command still ends with its code.
#[tokio::test(flavor = "multi_thread")]
async fn large_output_is_drained_while_the_command_runs() {
    let root = common::temp("toolkit-chatty");
    script(&root, "chatty", "/usr/bin/head -c 1048576 /dev/zero; /usr/bin/head -c 1048576 /dev/zero >&2; exit 3");
    let path = format!("{}:/usr/bin:/bin", root.display());
    let output = executor(environment(&[("PATH", &path)]), &root).run(&argv(&["chatty"])).await;
    assert_eq!(output.exit_code, 3);
    assert_eq!(output.output.len(), 2 * 1_048_576);
    common::remove(&root);
}

/// Swift `ToolkitProcessTests.swift:50 "aMissingExecutableIsAFailureNotACrash"`.
#[tokio::test(flavor = "multi_thread")]
async fn a_missing_executable_is_a_failure_not_a_crash() {
    let root = common::temp("toolkit-missing");
    let output = executor(environment(&[("PATH", "/nonexistent")]), &root).run(&argv(&["mighty-no-such-tool"])).await;
    assert_ne!(output.exit_code, 0);
    common::remove(&root);
}

/// Swift `ToolkitProcessTests.swift:55 "aCommandPastItsDeadlineIsStoppedWithItsGroup"`: a child of the command, in its group, does not
/// outlive the stop.
#[tokio::test(flavor = "multi_thread")]
async fn a_command_past_its_deadline_is_stopped_with_its_group() {
    let root = common::temp("toolkit-slow");
    let pid = root.join("child.pid");
    script(&root, "slow", &format!("/bin/sleep 60 & echo $! > '{}'; wait", pid.display()));
    let (ran, took) = run("slow", &format!("{}:/usr/bin:/bin", root.display()), &root, Duration::from_secs(3)).await;
    assert!(took < Duration::from_secs(15), "{took:?}");
    assert!(ran.timed_out && ran.exit_code != 0, "{ran:?}");
    let child: libc::pid_t = std::fs::read_to_string(&pid).unwrap().trim().parse().unwrap();
    let began = Instant::now();
    // SAFETY: signal 0 only checks.
    while unsafe { libc::kill(child, 0) } == 0 && began.elapsed() < Duration::from_secs(5) {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    // SAFETY: as above.
    assert_ne!(unsafe { libc::kill(child, 0) }, 0, "the group was stopped");
    common::remove(&root);
}

/// Swift `ToolkitProcessTests.swift:72 "aBackgroundProcessHoldingThePipeIsNotWaitedFor"`: the command ends at once; what it left behind
/// keeps stdout open, and is not waited for.
#[tokio::test(flavor = "multi_thread")]
async fn a_background_process_holding_the_pipe_is_not_waited_for() {
    let root = common::temp("toolkit-leaves-child");
    script(&root, "leaves-child", "/bin/sleep 30 & echo done");
    let (ran, took) = run("leaves-child", &format!("{}:/usr/bin:/bin", root.display()), &root, Duration::from_secs(60)).await;
    assert!(took < Duration::from_secs(10), "{took:?}");
    assert!(ran.exit_code == 0 && ran.output.starts_with("done"), "{ran:?}");
    common::remove(&root);
}
