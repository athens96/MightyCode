//! W9-E: the waitable detached start (`ProcessHost::start_detached_waitable`, C# `OuroborosDashboardClient.Capture`: `Process.Start`,
//! `WaitForExitAsync`, `Kill(entireProcessTree: false)`) on the Windows host, by real processes: the exact environment, the exit code,
//! and an end that leaves what the process started running.
#![cfg(windows)]

mod common;

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use common::*;
use mighty_platform::{DetachedEnvironment, DetachedRequest, ProcessHost};
use mighty_platform_windows::{KILLED_EXIT_CODE, WindowsProcessHost};

fn request(args: &[&str], environment: DetachedEnvironment) -> DetachedRequest {
    DetachedRequest {
        program: fake(),
        args: args.iter().map(|a| a.to_string()).collect(),
        cwd: std::env::temp_dir(),
        environment,
        hidden: true,
    }
}

/// The child reads exactly the environment given and its exit code is read back.
#[tokio::test(flavor = "multi_thread")]
async fn a_waitable_detached_start_gets_exactly_its_environment_and_is_waited_for() {
    let host = WindowsProcessHost::new();
    let directory = temp("detached-env");
    let file = directory.join("env.txt");
    let mut values = BTreeMap::from([("MIGHTY_DETACHED".to_string(), "yes".to_string())]);
    if let Ok(root) = std::env::var("SystemRoot") {
        values.insert("SystemRoot".to_string(), root);
    }
    let child =
        host.start_detached_waitable(request(&["--print-env", &file.to_string_lossy()], DetachedEnvironment::Exactly(values))).unwrap();
    assert!(child.id() > 0, "a process id");
    assert_eq!(tokio::time::timeout(Duration::from_secs(20), child.wait()).await.unwrap().unwrap(), 0, "the exit code is read");
    let printed = std::fs::read_to_string(&file).unwrap();
    let names: Vec<&str> =
        printed.split('\0').filter_map(|entry| entry.split_once('=').map(|(name, _)| name)).filter(|n| !n.is_empty()).collect();
    assert!(printed.contains("MIGHTY_DETACHED=yes"), "the variable given: {printed}");
    assert!(!names.iter().any(|n| n.eq_ignore_ascii_case("NO_COLOR") || n.eq_ignore_ascii_case("PATH")), "nothing else: {names:?}");
    let failed = host.start_detached_waitable(request(&["--fail-child", "no"], DetachedEnvironment::Inherit)).unwrap();
    assert_eq!(tokio::time::timeout(Duration::from_secs(20), failed.wait()).await.unwrap().unwrap(), 3, "a failing exit code");
    let _ = std::fs::remove_dir_all(directory);
}

/// Ending the start ends it alone: the process it started keeps running (C#'s `Kill(entireProcessTree: false)`).
#[tokio::test(flavor = "multi_thread")]
async fn ending_a_waitable_detached_start_leaves_what_it_started() {
    let host = WindowsProcessHost::new();
    let directory = temp("detached-end");
    let pid_file = directory.join("grandchild.pid");
    let child =
        host.start_detached_waitable(request(&["--group-child", &pid_file.to_string_lossy()], DetachedEnvironment::Inherit)).unwrap();
    let started = Instant::now();
    while !pid_file.exists() {
        assert!(started.elapsed() < Duration::from_secs(20), "the grandchild started");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let grandchild: u32 = std::fs::read_to_string(&pid_file).unwrap().trim().parse().unwrap();
    assert!(alive(child.id()) && alive(grandchild), "both run");
    child.terminate();
    assert_eq!(tokio::time::timeout(Duration::from_secs(10), child.wait()).await.unwrap().unwrap(), KILLED_EXIT_CODE, "the start is ended");
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(alive(grandchild), "what it started keeps running");
    child.terminate();
    // The fixture's grandchild ends on its own after 60 s; end it now.
    let end = host.start_detached_waitable(DetachedRequest {
        program: std::path::PathBuf::from(std::env::var("SystemRoot").unwrap()).join("System32").join("taskkill.exe"),
        args: vec!["/F".into(), "/PID".into(), grandchild.to_string()],
        cwd: std::env::temp_dir(),
        environment: DetachedEnvironment::Inherit,
        hidden: true,
    });
    if let Ok(end) = end {
        let _ = tokio::time::timeout(Duration::from_secs(10), end.wait()).await;
    }
    let _ = std::fs::remove_dir_all(directory);
}
