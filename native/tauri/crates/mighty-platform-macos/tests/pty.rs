//! The Mac PTY host by real terminals (Swift `PTYAgentTerminalPane`'s launch, typing, resize and stop; `ShellActivityTests`): the
//! counterpart of mighty-platform-windows' pseudo_terminal.rs.
#![cfg(target_os = "macos")]

mod common;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use common::*;
use mighty_platform::{PtyHost, PtyRequest, PtySession, PtySize};
use mighty_platform_macos::pty::{MacPtyHost, has_children};

fn environment() -> BTreeMap<String, String> {
    std::env::vars().collect()
}

async fn open(program: &str, args: &[&str], cwd: &Path) -> (PtySession, Arc<Mutex<Vec<u8>>>) {
    let request = PtyRequest {
        program: PathBuf::from(program),
        args: args.iter().map(|a| a.to_string()).collect(),
        cwd: cwd.to_path_buf(),
        env: environment(),
        size: PtySize::default(),
    };
    let mut session = MacPtyHost.open(request).await.unwrap();
    let output = Arc::new(Mutex::new(Vec::new()));
    let sink = output.clone();
    let mut receiver = std::mem::replace(&mut session.output, tokio::sync::mpsc::channel(1).1);
    tokio::spawn(async move {
        while let Some(bytes) = receiver.recv().await {
            sink.lock().unwrap().extend_from_slice(&bytes);
        }
    });
    (session, output)
}

fn text(output: &Arc<Mutex<Vec<u8>>>) -> String {
    String::from_utf8_lossy(&output.lock().unwrap()).into_owned()
}

/// A persistent interactive shell: typed lines run, a resize reaches `stty`, `busy` reads the foreground group (idle at the prompt,
/// busy while a command runs), the session's processes are listed, and the close ends it and lets its output end.
#[tokio::test(flavor = "multi_thread")]
async fn a_persistent_shell_runs_typed_lines_resizes_and_closes() {
    let folder = temp("shell");
    let (session, output) = open("/bin/zsh", &["-f"], &folder).await;
    let terminal = session.control.clone();
    assert!(terminal.write(b"PS1='mighty$ '; echo MARK$((6*7))\r"));
    until(|| text(&output).contains("MARK42"), 20_000, "the typed line ran").await;
    until(|| terminal.busy() == Some(false), 10_000, "idle at the prompt").await;
    assert!(terminal.process_ids().is_some_and(|ids| ids.contains(&session.id)), "the shell is one of the session's processes");
    terminal.resize(PtySize { columns: 91, rows: 23 }).unwrap();
    assert!(terminal.write(b"stty size; tty\r"));
    until(|| text(&output).contains("23 91"), 10_000, "the grid reached the terminal").await;
    until(|| text(&output).contains("/dev/ttys"), 10_000, "tty named the terminal").await;
    assert!(terminal.write(b"sleep 3\r"));
    until(|| terminal.busy() == Some(true), 10_000, "busy while a command owns the terminal").await;
    until(|| terminal.busy() == Some(false), 20_000, "idle again after it").await;
    let long_text = "가".repeat(65_537);
    assert!(!terminal.write(long_text.as_bytes()), "more than 65,536 UTF-16 units at once is refused");
    terminal.close().await.unwrap();
    terminal.close().await.unwrap();
    assert!(!terminal.write(b"late\r"), "a closed terminal takes nothing");
    assert_eq!(terminal.busy(), None, "nothing is known once closed");
    tokio::time::timeout(Duration::from_secs(10), terminal.wait()).await.expect("the shell ended").unwrap();
    remove(&folder);
}

/// The command's exit code comes back; one ended by a signal reads as 128 + the signal (a typed Ctrl+C reaches the terminal's
/// foreground group: Swift "realPaneTypedCtrlCInterruptsTheRunningCommand"); the terminal is UTF-8 (`IUTF8`) and named
/// `xterm-256color`; the command runs in the request's folder.
#[tokio::test(flavor = "multi_thread")]
async fn exit_codes_signals_and_the_terminal_the_command_sees() {
    let folder = temp("codes");
    let (session, output) = open("/bin/sh", &["-c", "pwd -P; echo $TERM; stty -a | tr ' ' '\\n' | grep -x iutf8; exit 3"], &folder).await;
    assert_eq!(tokio::time::timeout(Duration::from_secs(20), session.control.wait()).await.unwrap().unwrap(), 3);
    session.control.close().await.unwrap();
    until(|| text(&output).contains("iutf8"), 10_000, "the last line was drained").await;
    let printed = text(&output);
    assert!(printed.contains(&format!("{}\r\n", folder.display())), "{printed:?}");
    assert!(printed.contains("xterm-256color\r\n") && printed.contains("iutf8\r\n"), "{printed:?}");

    let (session, output) = open("/bin/sh", &["-c", "echo started; exec sleep 30"], &folder).await;
    until(|| text(&output).contains("started"), 20_000, "the command started").await;
    assert!(session.control.write(b"\x03"));
    assert_eq!(tokio::time::timeout(Duration::from_secs(20), session.control.wait()).await.unwrap().unwrap(), 128 + libc::SIGINT);
    session.control.close().await.unwrap();
    remove(&folder);
}

/// Closing the terminal ends every process of its session at once, a background job and one ignoring SIGINT and SIGTERM included
/// (the kill-on-close job's counterpart), whether or not anyone reads the output.
#[tokio::test(flavor = "multi_thread")]
async fn the_close_ends_every_process_of_the_session() {
    let folder = temp("close");
    let request = PtyRequest {
        program: "/bin/sh".into(),
        args: vec!["-c".into(), "trap '' INT TERM; /bin/sleep 30 & echo $! > child.pid; yes | head -c 100000000; wait".into()],
        cwd: folder.clone(),
        env: environment(),
        size: PtySize::default(),
    };
    let session = MacPtyHost.open(request).await.unwrap();
    let child = pid_from(&folder.join("child.pid")).await;
    assert!(alive(session.id) && alive(child));
    assert!(has_children(session.id as libc::pid_t), "the shell has its background child");
    // Nobody reads the output: the close must not wait on it.
    tokio::time::timeout(Duration::from_secs(10), session.control.close()).await.expect("the close returns").unwrap();
    until(|| !alive(child) && !alive(session.id), 10_000, "the session's processes are gone").await;
    remove(&folder);
}

/// Swift `ShellActivityTests.swift:124 "aShellHoldingABackgroundJobHasChildrenAndALoneProcessDoesNot"`.
#[tokio::test(flavor = "multi_thread")]
async fn a_shell_holding_a_background_job_has_children_and_a_lone_process_does_not() {
    let folder = temp("children");
    let (holding, output) = open("/bin/sh", &["-c", "/bin/sleep 30 & echo held; wait"], &folder).await;
    until(|| text(&output).contains("held"), 20_000, "the job started").await;
    assert!(has_children(holding.id as libc::pid_t));
    let (lone, lone_output) = open("/bin/sh", &["-c", "echo alone; exec /bin/sleep 30"], &folder).await;
    until(|| text(&lone_output).contains("alone"), 20_000, "the lone process runs").await;
    assert!(!has_children(lone.id as libc::pid_t));
    assert!(!has_children(0) && !has_children(-5), "no reading of nothing");
    holding.control.close().await.unwrap();
    lone.control.close().await.unwrap();
    remove(&folder);
}

/// What replaces Swift `ShellActivityTests.swift:142 "aPidFileCountsOnlyForAProcessBelowTheApp"` (not ported: the Mac host keeps no
/// pid file). Swift named the shell through a pid file because Ghostty owned the PTY, and counted it only for a process below the
/// app; the app owns the PTY now, so the terminal's processes are the session it opened: its shell, and never this process, its
/// parent, launchd or an id that runs nothing.
#[tokio::test(flavor = "multi_thread")]
async fn only_the_opened_sessions_processes_count() {
    use mighty_platform_macos::pty::session_members;
    let folder = temp("below");
    let (session, output) = open("/bin/sh", &["-c", "echo up; exec /bin/sleep 20"], &folder).await;
    until(|| text(&output).contains("up"), 20_000, "the shell runs").await;
    assert_eq!(session_members(session.id as libc::pid_t), [session.id]);
    // SAFETY: plain reads of this process's ids.
    let (me, parent) = unsafe { (libc::getpid(), libc::getppid()) };
    for other in [me, parent, 1, 0, -4] {
        assert!(!session_members(session.id as libc::pid_t).contains(&(other as u32)), "{other}");
    }
    assert!(session_members(999_999).is_empty(), "an id that runs nothing has no session");
    session.control.close().await.unwrap();
    remove(&folder);
}

/// A process that left the session (its own `setsid`) but keeps writing to the terminal never holds the close: the drain reads on for
/// a second after the close began, then lets go.
#[tokio::test(flavor = "multi_thread")]
async fn a_writer_outside_the_session_never_holds_the_close() {
    let folder = temp("stray-writer");
    let script = concat!(
        r#"/usr/bin/perl -e 'use POSIX qw(setsid); exit 0 if fork; setsid(); open(my $f, ">", "stray.pid"); print $f $$; close($f); "#,
        r#"$| = 1; while (1) { print "x" x 1000, "\n"; select(undef, undef, undef, 0.005) }'; echo started; exec /bin/sleep 30"#,
    );
    let (session, output) = open("/bin/sh", &["-c", script], &folder).await;
    let pid = pid_from(&folder.join("stray.pid")).await;
    until(|| text(&output).contains("started"), 20_000, "the shell runs").await;
    let began = std::time::Instant::now();
    tokio::time::timeout(Duration::from_secs(15), session.control.close()).await.expect("the close returns").unwrap();
    assert!(began.elapsed() < Duration::from_secs(10), "{:?}", began.elapsed());
    // SAFETY: ends the fixture's stray writer, which left the session on purpose.
    unsafe { libc::kill(pid as libc::pid_t, libc::SIGKILL) };
    remove(&folder);
}
