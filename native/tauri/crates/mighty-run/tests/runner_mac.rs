//! Swift's runner checks that need the Mac process host's own behaviour (`RunnerBackpressureTests.swift`). Mac only: the stand-in agy
//! is a `/bin/sh` script, as Swift's, and the stray writer leaves the child's process group with `set -m`.
#![cfg(target_os = "macos")]

mod common;

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use mighty_core::model::Workspace;
use mighty_run::manager::ResolveWorkspace;
use mighty_run::model::{CliCommand, RunEvent, StartRunRequest};
use mighty_run::{RunManager, RunManagerOptions};

fn workspace(directory: &Path) -> Workspace {
    Workspace { path: Some(directory.to_string_lossy().into_owned()), ..Workspace::default() }
}

fn resolve(workspace: &Workspace) -> ResolveWorkspace {
    let workspace = workspace.clone();
    Arc::new(move |_| {
        let workspace = workspace.clone();
        Box::pin(async move { Ok(workspace) })
    })
}

/// Swift `RunnerBackpressureTests.swift:369 "aDetachedWriterCannotKeepTheRunAliveAfterExit"`: a writer outside the child's process
/// group keeps the inherited stdout open after the child exits; the run still ends, and the writer loses its pipe. (Swift also posts a
/// `run.notice.outputAbandoned` line; the Rust run, C#'s, has no such notice: the host's reader ends the pipe a second after the exit
/// and the run completes as the CLI's own exit says.)
#[tokio::test(flavor = "multi_thread")]
async fn a_detached_writer_cannot_keep_the_run_alive_after_exit() {
    use std::os::unix::fs::PermissionsExt as _;
    common::korean();
    let directory = common::temp("detached-writer");
    let binary = directory.join("agy");
    let source = r#"#!/bin/sh
if [ "$1" = "--version" ]; then printf '0.20.0\n'; exit 0; fi
/bin/cat > /dev/null
printf '%s\n' '{"event":"init","conversation_id":"fixture-session"}'
pad=$(/usr/bin/awk 'BEGIN { s = sprintf("%2000s", ""); gsub(/ /, "x", s); print s }')
set -m
/usr/bin/yes "{\"event\":\"step_update\",\"step_update\":{\"step_index\":1,\"state\":\"DONE\",\"step_type\":\"agent_response\",\"text_delta\":\"$pad\"}}" &
echo $! > "$(/usr/bin/dirname "$0")/stray.pid"
exit 0
"#;
    std::fs::write(&binary, source).unwrap();
    std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o700)).unwrap();
    let command = CliCommand::new(binary.to_string_lossy().into_owned(), vec![], Some("0.20.0"));
    let catalog = common::catalog(move |_| Some(command.clone()));
    let events: Arc<Mutex<Vec<RunEvent>>> = Arc::default();
    let sink = events.clone();
    let workspace = workspace(&directory);
    let manager =
        RunManager::new(catalog, RunManagerOptions::new("", resolve(&workspace), Arc::new(move |event| sink.lock().unwrap().push(event))));
    let ended = |events: &Arc<Mutex<Vec<RunEvent>>>| {
        events.lock().unwrap().iter().rev().find(|e| e.kind == "status").and_then(|e| e.status.clone()).filter(|s| s != "running")
    };
    let began = Instant::now();
    manager.start(StartRunRequest::new("pane", workspace.id.as_deref().unwrap(), "claude", "go").with_provider("gemini")).await.unwrap();
    common::until(|| ended(&events).is_some(), 25_000, "the run ends").await;
    assert!(began.elapsed() < Duration::from_secs(25));
    assert_eq!(ended(&events).as_deref(), Some("completed"));
    let stray: u32 = std::fs::read_to_string(directory.join("stray.pid")).unwrap().trim().parse().unwrap();
    // Its pipe is closed: the next write fails (SIGPIPE / EPIPE) and it ends.
    common::until(|| !common::alive(stray), 20_000, "the stray writer ends").await;
    manager.dispose().await;
    common::remove_when_free(&directory).await;
}

/// Swift `CoreTests.swift:255 "testShellRunnerRunsStopsAndKeepsEventsIsolated"`: two shell panes' commands run through `/bin/sh -c`;
/// the first completes with its Korean output, the second is stopped while it sleeps, and neither's events reach the other.
#[tokio::test(flavor = "multi_thread")]
async fn test_shell_runner_runs_stops_and_keeps_events_isolated() {
    common::korean();
    let directory = common::temp("shell-isolated");
    let events: Arc<Mutex<Vec<RunEvent>>> = Arc::default();
    let sink = events.clone();
    let workspace = workspace(&directory);
    let manager = RunManager::new(
        common::absent_catalog(),
        RunManagerOptions::new("", resolve(&workspace), Arc::new(move |event| sink.lock().unwrap().push(event))),
    );
    let id = workspace.id.clone().unwrap();
    let has = |session: &str, test: &dyn Fn(&RunEvent) -> bool| events.lock().unwrap().iter().any(|e| e.session_id == session && test(e));
    let text = |e: &RunEvent| e.entry.as_ref().and_then(|l| l.text.clone()).unwrap_or_default();
    manager.start(StartRunRequest::new("first", &id, "shell", "printf '첫 번째'")).await.unwrap();
    // The run engine (C#'s) hands a shell's output on by lines, so the second's marker ends its line (`echo`; Swift's `printf READY`
    // was handed on as it came).
    manager.start(StartRunRequest::new("second", &id, "shell", "echo READY; sleep 30")).await.unwrap();
    common::until(|| has("first", &|e| e.status.as_deref() == Some("completed")), 20_000, "the first completes").await;
    common::until(|| has("second", &|e| text(e) == "READY"), 20_000, "the second says READY").await;
    manager.stop("second").await.unwrap();
    assert!(has("second", &|e| e.status.as_deref() == Some("stopped")));
    assert!(has("first", &|e| text(e) == "첫 번째"));
    assert!(!has("first", &|e| text(e) == "READY") && !has("second", &|e| text(e) == "첫 번째"), "events stay with their pane");
    manager.dispose().await;
    common::remove_when_free(&directory).await;
}
