//! The Mac host's detached starts by real processes (the counterpart of mighty-platform-windows' detached_waitable.rs): what
//! `ProcessHost::start_detached` and `start_detached_waitable` promise. The CI source guard lets this file call them (native-tauri.yml).
#![cfg(target_os = "macos")]

mod common;

use std::collections::BTreeMap;
use std::time::Duration;

use common::*;
use mighty_platform::{DetachedEnvironment, DetachedRequest, ProcessHost};
use mighty_platform_macos::process::MacProcessHost;

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
        args: vec![
            "-c".into(),
            "/usr/bin/env > env.txt; pwd -P > pwd.txt; /usr/sbin/lsof -a -p $$ -d 0,1,2 -Fn > fds.txt; echo $$ > done.pid; /bin/sleep 3"
                .into(),
        ],
        cwd: directory.clone(),
        environment: DetachedEnvironment::Exactly(environment),
        hidden: true,
    };
    let pid = host.start_detached(request).unwrap();
    let written = pid_from(&directory.join("done.pid")).await;
    assert_eq!(written, pid);
    // SAFETY: reads a process's session while it sleeps.
    assert_eq!(unsafe { libc::getsid(pid as libc::pid_t) }, pid as libc::pid_t, "it leads a session of its own");
    let fds = std::fs::read_to_string(directory.join("fds.txt")).unwrap();
    let names: Vec<&str> = fds.lines().filter(|line| line.starts_with('n')).collect();
    assert_eq!(names, ["n/dev/null"; 3], "its standard streams are /dev/null: {fds:?}");
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
