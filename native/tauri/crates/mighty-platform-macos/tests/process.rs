//! The Mac process host by real processes: what the port's contract (mighty-platform `process`) and the fake's (tests/ports.rs)
//! state, the counterparts of mighty-platform-windows' child_process.rs and detached_waitable.rs, and Swift's process checks
//! (`CoreTests.swift`).
#![cfg(target_os = "macos")]

mod common;

use std::collections::BTreeMap;
use std::path::Path;
use std::time::{Duration, Instant};

use common::*;
use mighty_platform::{DetachedEnvironment, DetachedRequest, PlatformError, ProcessHost, SpawnRequest, capture};
use mighty_platform_macos::process::MacProcessHost;
use tokio::io::AsyncWriteExt;

fn environment_of(text: &str) -> Vec<(String, String)> {
    text.split('\0').filter_map(|entry| entry.split_once('=')).map(|(name, value)| (name.to_string(), value.to_string())).collect()
}

async fn printed_environment(host: &MacProcessHost, request: SpawnRequest) -> Vec<(String, String)> {
    let captured = capture(host, request, None, Duration::from_secs(10), 1 << 20).await.unwrap();
    assert_eq!(captured.exit_code, 0);
    environment_of(&String::from_utf8_lossy(&captured.stdout))
}

/// The environment `ChildProcess.StartInfo` gives: this process's with the request's laid over it (names compared exactly, as the
/// Mac does), an empty value set and empty, `NO_COLOR=1`.
#[tokio::test(flavor = "multi_thread")]
async fn the_child_environment_is_the_apps_with_the_request_laid_over_it() {
    let host = MacProcessHost::new();
    let directory = temp("env");
    let request = SpawnRequest::new("/usr/bin/env", ["-0"], &directory)
        .env("PATH", "/override")
        .env("path", "lower")
        .env("MIGHTY_EMPTY", "")
        .env("MIGHTY_VALUE", "한글 = value");
    let entries = printed_environment(&host, request).await;
    let get = |name: &str| entries.iter().filter(|(n, _)| n == name).map(|(_, v)| v.as_str()).collect::<Vec<_>>();
    assert_eq!(get("PATH"), ["/override"]);
    assert_eq!(get("path"), ["lower"], "another case is another name on the Mac");
    assert_eq!(get("MIGHTY_EMPTY"), [""], "an empty value stays set");
    assert_eq!(get("MIGHTY_VALUE"), ["한글 = value"]);
    assert_eq!(get("NO_COLOR"), ["1"]);
    assert_eq!(get("HOME").len(), 1, "the app's own environment is inherited");
    remove(&directory);
}

/// `env_remove` takes names out before `env` is laid over it (`WorkspaceGitInfo.Command` clears the app's `GIT_*` names): a removed
/// name is gone, one set again has its new value; `cli_request` runs the program directly and `shell_request` runs `/bin/sh -c`.
#[tokio::test(flavor = "multi_thread")]
async fn a_removed_variable_is_not_inherited_and_one_set_again_has_its_new_value() {
    let host = MacProcessHost::new();
    let directory = temp("env-remove");
    assert!(std::env::var_os("HOME").is_some() && std::env::var_os("PATH").is_some(), "the test reads two inherited names");
    let request =
        SpawnRequest::new("/usr/bin/env", ["-0"], &directory).without("HOME").without("PATH").env("PATH", "/again").without("home");
    let entries = printed_environment(&host, request).await;
    assert!(!entries.iter().any(|(name, _)| name == "HOME"), "a removed name is gone");
    assert!(entries.contains(&("PATH".into(), "/again".into())), "a name set again has its new value");
    assert!(host.spawn(SpawnRequest::new("/bin/echo", ["x"], &directory).without("")).await.is_err(), "an empty name is refused");

    let cli = host.cli_request(Path::new("/bin/echo"), &["hi".to_string()], &directory, &BTreeMap::new()).unwrap();
    assert!(cli.shell_command.is_none() && cli.program == Path::new("/bin/echo") && cli.args == ["hi"]);
    assert_eq!(cli.env.get("NO_COLOR").map(String::as_str), Some("1"));
    let shell = host.shell_request("echo 안녕; echo $0", &directory, &BTreeMap::new());
    assert_eq!((shell.program.as_path(), shell.args[0].as_str()), (Path::new("/bin/sh"), "-c"));
    let captured = capture(&host, shell, None, Duration::from_secs(10), 4096).await.unwrap();
    assert_eq!(String::from_utf8_lossy(&captured.stdout), "안녕\n/bin/sh\n");
    remove(&directory);
}

#[tokio::test(flavor = "multi_thread")]
async fn stdin_reaches_the_child_and_its_shutdown_is_the_end_of_input() {
    let host = MacProcessHost::new();
    let directory = temp("cat");
    let mut child = host.spawn(SpawnRequest::new("/bin/cat", Vec::<String>::new(), &directory)).await.unwrap();
    let mut input = child.stdin.take().unwrap();
    input.write_all("hello 한글\n".as_bytes()).await.unwrap();
    input.shutdown().await.unwrap();
    assert!(input.write_all(b"late").await.is_err(), "a closed stdin takes nothing more");
    let output = tokio::time::timeout(Duration::from_secs(10), read_all(child.stdout.take())).await.unwrap();
    assert_eq!(output, "hello 한글\n".as_bytes());
    assert_eq!(child.wait().await.unwrap(), 0);
    child.close().await.unwrap();
    remove(&directory);
}

/// A relative path, a NUL, a name holding `=` and Windows' cmd.exe line are refused before anything starts; a missing program is
/// the OS's "not found"; the working folder is the request's.
#[tokio::test(flavor = "multi_thread")]
async fn relative_paths_nul_bytes_and_missing_programs_are_refused() {
    let host = MacProcessHost::new();
    let directory = temp("refuse");
    assert!(matches!(host.spawn(SpawnRequest::new("sh", ["-c", "true"], &directory)).await, Err(PlatformError::Invalid(_))));
    assert!(matches!(host.spawn(SpawnRequest::new("/bin/sh", ["-c", "true"], "relative")).await, Err(PlatformError::Invalid(_))));
    assert!(matches!(host.spawn(SpawnRequest::new("/bin/echo", ["a\0b"], &directory)).await, Err(PlatformError::Invalid(_))));
    assert!(matches!(host.spawn(SpawnRequest::new("/bin/echo", ["x"], &directory).env("A=B", "x")).await, Err(PlatformError::Invalid(_))));
    let mut windows = SpawnRequest::new("/bin/echo", ["x"], &directory);
    windows.shell_command = Some("echo x".into());
    assert!(matches!(host.spawn(windows).await, Err(PlatformError::Invalid(_))));
    match host.spawn(SpawnRequest::new(directory.join("missing"), ["x"], &directory)).await {
        Err(PlatformError::Io(error)) => assert_eq!(error.kind(), std::io::ErrorKind::NotFound),
        other => panic!("a missing program: {other:?}"),
    }
    let captured = capture(&host, SpawnRequest::new("/bin/pwd", ["-P"], &directory), None, Duration::from_secs(10), 4096).await.unwrap();
    assert_eq!(String::from_utf8_lossy(&captured.stdout).trim(), directory.to_string_lossy());
    remove(&directory);
}

/// The child leads a process group of its own; `kill` is SIGKILL to it (the exit code 128 + 9), and closes after it, or a second
/// close, do nothing.
#[tokio::test(flavor = "multi_thread")]
async fn a_killed_child_reports_sigkill_and_a_second_close_does_nothing() {
    let host = MacProcessHost::new();
    let directory = temp("kill");
    let child = host.spawn(SpawnRequest::new("/bin/sleep", ["30"], &directory)).await.unwrap();
    // SAFETY: reads a process's group.
    assert_eq!(unsafe { libc::getpgid(child.id as libc::pid_t) }, child.id as libc::pid_t, "the child leads its own group");
    let control = child.control();
    child.kill();
    assert_eq!(tokio::time::timeout(Duration::from_secs(10), control.wait()).await.unwrap().unwrap(), 137);
    control.close().await.unwrap();
    control.close().await.unwrap();
    control.kill();
    drop(child);
    remove(&directory);
}

/// The grandchild dies with the group: a shell's background `sleep` is ended by the close, and the close returns only once it is.
#[tokio::test(flavor = "multi_thread")]
async fn a_grandchild_dies_when_the_parents_group_is_closed() {
    let host = MacProcessHost::new();
    let directory = temp("grandchild");
    let pid = directory.join("child.pid");
    let child = host.spawn(SpawnRequest::new("/bin/sh", ["-c", "/bin/sleep 30 & echo $! > child.pid; wait"], &directory)).await.unwrap();
    let grandchild = pid_from(&pid).await;
    assert!(alive(child.id) && alive(grandchild));
    let parent = child.id;
    child.close().await.unwrap();
    assert!(!alive(grandchild), "the close must end the grandchild before it returns");
    assert!(!alive(parent));
    remove(&directory);
}

/// No descriptor of the app's reaches the child but its three pipes (`POSIX_SPAWN_CLOEXEC_DEFAULT`), even one opened without
/// close-on-exec.
#[tokio::test(flavor = "multi_thread")]
async fn only_the_three_pipes_are_inherited() {
    let host = MacProcessHost::new();
    let directory = temp("descriptors");
    // SAFETY: duplicates standard error to a high descriptor without close-on-exec; closed below.
    let leaked = unsafe { libc::fcntl(libc::STDERR_FILENO, libc::F_DUPFD, 200) };
    assert!(leaked >= 200);
    let check = format!("[ -e /dev/fd/{leaked} ] && echo leaked || echo clean");
    let captured =
        capture(&host, SpawnRequest::new("/bin/sh", ["-c", check.as_str()], &directory), None, Duration::from_secs(10), 4096).await;
    // SAFETY: the descriptor made above.
    unsafe { libc::close(leaked) };
    assert_eq!(String::from_utf8_lossy(&captured.unwrap().stdout).trim(), "clean");
    remove(&directory);
}

/// Swift `CoreTests.swift:232 "testProcessCaptureHandlesStdinTimeoutAndOutputLimits"`: a capture feeds stdin; past its timeout the
/// child is stopped well before it would end; past its output limit the capture fails.
#[tokio::test(flavor = "multi_thread")]
async fn test_process_capture_handles_stdin_timeout_and_output_limits() {
    let host = MacProcessHost::new();
    let directory = temp("capture");
    let cat = SpawnRequest::new("/bin/cat", Vec::<String>::new(), &directory);
    let captured = capture(&host, cat, Some("한국어 stdin\n".as_bytes().to_vec()), Duration::from_secs(5), 2 << 20).await.unwrap();
    assert_eq!((captured.exit_code, String::from_utf8_lossy(&captured.stdout).as_ref()), (0, "한국어 stdin\n"));
    let start = Instant::now();
    let slow = SpawnRequest::new("/bin/sh", ["-c", "sleep 30"], &directory);
    assert!(matches!(capture(&host, slow, None, Duration::from_millis(150), 2 << 20).await, Err(PlatformError::TimedOut(_))));
    assert!(start.elapsed() < Duration::from_secs(10), "the timeout is enforced well before the child would end");
    let yes = SpawnRequest::new("/usr/bin/yes", Vec::<String>::new(), &directory);
    assert!(matches!(capture(&host, yes, None, Duration::from_secs(3), 1024).await, Err(PlatformError::TooLarge(1024))));
    remove(&directory);
}

/// Swift `CoreTests.swift:245 "testProcessGroupCleansBackgroundChildWhenParentExits"`: a background child of the capture's shell is
/// ended once the shell exits, so its hold on the pipe does not keep the capture waiting.
#[tokio::test(flavor = "multi_thread")]
async fn test_process_group_cleans_background_child_when_parent_exits() {
    let host = MacProcessHost::new();
    let directory = temp("group-clean");
    let request = SpawnRequest::new("/bin/sh", ["-c", "sleep 30 & echo $! > child.pid; printf PARENT_DONE"], &directory);
    let captured = capture(&host, request, None, Duration::from_secs(20), 1 << 20).await.unwrap();
    assert_eq!(String::from_utf8_lossy(&captured.stdout), "PARENT_DONE");
    let pid = pid_from(&directory.join("child.pid")).await;
    until(|| !alive(pid), 10_000, "the background child is ended with its group").await;
    remove(&directory);
}

/// A process outside the child's group (`set -m` gives a background job its own) keeps the inherited stdout open after the child
/// exits: the pipe is read for a second more, then the app's end closes, the reader sees its end, and the writer loses its pipe.
#[tokio::test(flavor = "multi_thread")]
async fn a_writer_outside_the_group_loses_its_pipe_after_the_exit() {
    let host = MacProcessHost::new();
    let directory = temp("stray");
    let script = "set -m; /usr/bin/yes stray & echo $! > stray.pid; exit 0";
    let mut child = host.spawn(SpawnRequest::new("/bin/sh", ["-c", script], &directory)).await.unwrap();
    let began = Instant::now();
    let output = tokio::time::timeout(Duration::from_secs(25), read_all(child.stdout.take())).await.expect("the output ends");
    assert!(began.elapsed() < Duration::from_secs(25) && !output.is_empty());
    assert_eq!(child.wait().await.unwrap(), 0);
    let stray = pid_from(&directory.join("stray.pid")).await;
    until(|| !alive(stray), 10_000, "the stray writer's next write fails and it ends").await;
    child.close().await.unwrap();
    remove(&directory);
}

/// `start_detached`: out of the app's session (it leads its own), standard streams on /dev/null, the environment exactly as given, in
/// the request's folder; it outlives nothing of the app's and is reaped when it ends.
#[tokio::test(flavor = "multi_thread")]
async fn a_detached_process_has_its_own_session_and_exactly_its_environment() {
    let host = MacProcessHost::new();
    let directory = temp("detached");
    let mut environment = BTreeMap::new();
    environment.insert("MIGHTY_ONLY".to_string(), "value".to_string());
    let request = DetachedRequest {
        program: "/bin/sh".into(),
        args: vec!["-c".into(), "/usr/bin/env > env.txt; pwd -P > pwd.txt; echo $$ > done.pid".into()],
        cwd: directory.clone(),
        environment: DetachedEnvironment::Exactly(environment),
        hidden: true,
    };
    let pid = host.start_detached(request).unwrap();
    let written = pid_from(&directory.join("done.pid")).await;
    assert_eq!(written, pid);
    let env = std::fs::read_to_string(directory.join("env.txt")).unwrap();
    let names: Vec<&str> = env.lines().filter_map(|line| line.split_once('=').map(|(name, _)| name)).collect();
    assert!(names.iter().all(|name| ["MIGHTY_ONLY", "PWD", "SHLVL", "_", "OLDPWD"].contains(name)), "{names:?}");
    assert!(env.contains("MIGHTY_ONLY=value"));
    assert_eq!(std::fs::read_to_string(directory.join("pwd.txt")).unwrap().trim(), directory.to_string_lossy());
    remove(&directory);
}

/// `start_detached_waitable`: its exit code is read; `terminate` ends it alone (its child keeps running), and after the exit does
/// nothing.
#[tokio::test(flavor = "multi_thread")]
async fn a_waitable_detached_process_is_waited_for_and_ended_alone() {
    let host = MacProcessHost::new();
    let directory = temp("waitable");
    let request = |script: &str| DetachedRequest {
        program: "/bin/sh".into(),
        args: vec!["-c".into(), script.into()],
        cwd: directory.clone(),
        environment: DetachedEnvironment::Inherit,
        hidden: true,
    };
    let quick = host.start_detached_waitable(request("exit 7")).unwrap();
    assert_eq!(tokio::time::timeout(Duration::from_secs(10), quick.wait()).await.unwrap().unwrap(), 7);
    quick.terminate();
    let parent = host.start_detached_waitable(request("/bin/sleep 30 & echo $! > child.pid; wait")).unwrap();
    // SAFETY: reads a process's session.
    assert_eq!(unsafe { libc::getsid(parent.id() as libc::pid_t) }, parent.id() as libc::pid_t, "it leads a session of its own");
    let child = pid_from(&directory.join("child.pid")).await;
    parent.terminate();
    assert_eq!(tokio::time::timeout(Duration::from_secs(10), parent.wait()).await.unwrap().unwrap(), 137);
    assert!(alive(child), "only the process itself is ended");
    // SAFETY: ends the fixture's leftover child.
    unsafe { libc::kill(child as libc::pid_t, libc::SIGKILL) };
    remove(&directory);
}

/// Swift `SetupPlanTests.swift:141 "controlCDuringTheWaitEndsOnlyTheWait"`, its process half: the child leads its own group as a
/// terminal's foreground job does, so a Control-C (SIGINT to the group) reaches the script and its `sleep` alike; the script's trap
/// ends only the wait, and it goes on to its next step and its end marker. (Swift renders the script with `SetupScript`; the Rust
/// setup script is Windows' PowerShell one, so the step bodies are written out here as Swift renders them.)
#[tokio::test(flavor = "multi_thread")]
async fn control_c_during_the_wait_ends_only_the_wait() {
    let host = MacProcessHost::new();
    let root = temp("setup-wait");
    let body = "trap 'break' INT; : > waiting; until false; do sleep 5; done; trap - INT\n: > next\n: > done\n";
    std::fs::write(root.join("setup.sh"), body).unwrap();
    let request = SpawnRequest::new("/bin/bash", ["setup.sh"], &root).env("PATH", "/usr/bin:/bin");
    let child = host.spawn(request).await.unwrap();
    until(|| root.join("waiting").exists(), 15_000, "the script waits").await;
    // SAFETY: SIGINT to the child's own group.
    assert_eq!(unsafe { libc::kill(-(child.id as libc::pid_t), libc::SIGINT) }, 0);
    let code = tokio::time::timeout(Duration::from_secs(15), child.wait()).await.expect("the script ended").unwrap();
    assert_eq!(code, 0);
    assert!(root.join("next").exists() && root.join("done").exists(), "the next step and the end marker still ran");
    child.close().await.unwrap();
    remove(&root);
}
