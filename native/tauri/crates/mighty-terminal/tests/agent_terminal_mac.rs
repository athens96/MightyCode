//! Swift's real-PTY checks of the agent pane's terminal (`TerminalPaneLifetimeTests`, `LongRunningProcessTests`,
//! `AgentTerminalToolTests`, `AgentTerminalToolOutputTests`), over the Mac PTY host. Swift's `AgentTerminalRunner` and the Rust
//! `AgentTerminal` keep one contract with two differences these checks follow: a stop is Ctrl+C, then (after 3 s) the command's whole
//! session ended at once, where Swift went SIGINT, SIGTERM (3 s), SIGKILL (5 s more); and what the agent reads starts with the pane's
//! `> <command>` line and ends with `[exit N]`. Mac only: the commands are POSIX shell lines run by the user's shell (Windows' real
//! check is agent_terminal.rs' `real_windows_interactive_process`).
#![cfg(target_os = "macos")]

mod common;

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use common::until;
use mighty_platform::fake::FakeShellEnvironment;
use mighty_terminal::AgentTerminal;
use mighty_terminal::agent::AgentTerminalResult;

fn folder(name: &str) -> std::path::PathBuf {
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
    let path = std::env::temp_dir().join(format!("mighty-agent-pty-{name}-{}-{nanos}", std::process::id()));
    std::fs::create_dir_all(&path).unwrap();
    std::fs::canonicalize(path).unwrap()
}

fn terminal(folder: &std::path::Path) -> Arc<AgentTerminal> {
    let environment = Arc::new(FakeShellEnvironment::new(std::env::vars().collect()));
    AgentTerminal::new(folder.to_path_buf(), mighty_platform_macos::pty::pty_host(), environment)
}

/// What the command printed, without the pane's `> <command>` line and the `[exit N]` line.
fn printed(result: &AgentTerminalResult) -> String {
    let lines: Vec<&str> = result.output.split('\n').filter(|line| !line.starts_with("> ") && !line.starts_with("[exit ")).collect();
    lines.join("\n").trim_matches('\n').to_string()
}

/// Reads until the command is done, every read's output together.
async fn finished(terminal: &Arc<AgentTerminal>, first: AgentTerminalResult) -> AgentTerminalResult {
    let mut output = first.output.clone();
    let mut last = first;
    for _ in 0..600 {
        if last.status == "done" {
            return AgentTerminalResult { output, ..last };
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
        last = terminal.read(&last.handle).unwrap();
        output.push_str(&last.output);
    }
    panic!("the command did not end: {output:?}");
}

/// Swift `TerminalPaneLifetimeTests.swift:104 "realPaneRunsTheCommandOnATTY"`.
#[tokio::test(flavor = "multi_thread")]
async fn real_pane_runs_the_command_on_a_tty() {
    let folder = folder("tty");
    let terminal = terminal(&folder);
    let first = terminal.run("tty; test -t 0 && test -t 1 && test -t 2 && echo all-tty").await.unwrap();
    let result = finished(&terminal, first).await;
    assert_eq!(result.exit_code, Some(0), "{result:?}");
    let printed = printed(&result);
    assert!(printed.starts_with("/dev/ttys") && printed.contains("all-tty"), "{printed:?}");
    terminal.dispose().await;
    let _ = std::fs::remove_dir_all(&folder);
}

/// Swift `TerminalPaneLifetimeTests.swift:113 "realPaneDeliversTypedTextToAReadingCommandAndTheAgentReadsIt"`: the typed line comes
/// back once, through the terminal's echo, then the command's answer; with nothing running, keystrokes are dropped.
#[tokio::test(flavor = "multi_thread")]
async fn real_pane_delivers_typed_text_to_a_reading_command_and_the_agent_reads_it() {
    let folder = folder("typed");
    let terminal = terminal(&folder);
    let handle = terminal.start("read x; echo got:$x", None).await.unwrap();
    assert!(terminal.write("hello\r"), "keystrokes go to the newest running command");
    assert_eq!(tokio::time::timeout(Duration::from_secs(20), terminal.exited(&handle)).await.unwrap(), Ok(Some(0)));
    let result = finished(&terminal, terminal.read(&handle).unwrap()).await;
    assert_eq!(printed(&result), "hello\ngot:hello");
    assert!(!terminal.write("late"), "with nothing running, keystrokes are dropped");
    terminal.dispose().await;
    let _ = std::fs::remove_dir_all(&folder);
}

/// Swift `TerminalPaneLifetimeTests.swift:131 "realPaneTypedCtrlCInterruptsTheRunningCommand"`: typed once the command runs, as a user
/// at the terminal would; it ends by SIGINT (128 + 2).
#[tokio::test(flavor = "multi_thread")]
async fn real_pane_typed_ctrl_c_interrupts_the_running_command() {
    let folder = folder("ctrl-c");
    let terminal = terminal(&folder);
    let handle = terminal.start("echo started; exec sleep 30", None).await.unwrap();
    until(|| terminal.read_output(&handle, 0, 1024).output.contains("started"), 20_000, "the command ran").await;
    assert!(terminal.write_to(&handle, "\u{3}"));
    assert_eq!(tokio::time::timeout(Duration::from_secs(20), terminal.exited(&handle)).await.unwrap(), Ok(Some(128 + 2)));
    terminal.dispose().await;
    let _ = std::fs::remove_dir_all(&folder);
}

/// Swift `TerminalPaneLifetimeTests.swift:162 "realProcessesSurviveTheTerminalPaneClosingAndReopening"`: the view letting go (its
/// listener unsubscribed) leaves the command running and still typed into; reopened, the pane replays everything; a stop ends it.
#[tokio::test(flavor = "multi_thread")]
async fn real_processes_survive_the_terminal_pane_closing_and_reopening() {
    let folder = folder("reopen");
    let terminal = terminal(&folder);
    let shown = Arc::new(Mutex::new(String::new()));
    let sink = shown.clone();
    let subscription = terminal.subscribe(Arc::new(move |text: &str| sink.lock().unwrap().push_str(text)));
    let handle = terminal.start("echo started; exec cat", None).await.unwrap();
    until(|| shown.lock().unwrap().contains("started\r\n"), 20_000, "the command's own line").await;
    terminal.unsubscribe(subscription);
    assert!(terminal.write_to(&handle, "while closed\r"));
    until(|| terminal.read_output(&handle, 0, 1024).output.contains("while closed\r\nwhile closed\r\n"), 20_000, "echo and cat").await;
    assert!(terminal.is_command_running(&handle) && !shown.lock().unwrap().contains("while closed"), "the closed view saw none of it");
    let replay = terminal.replay();
    assert!(replay.contains("> echo started; exec cat") && replay.contains("while closed\r\nwhile closed\r\n"), "{replay:?}");
    assert_eq!(terminal.read(&handle).unwrap().status, "running");
    let began = Instant::now();
    let stopped = terminal.stop(&handle).await.unwrap();
    assert!(stopped.status == "done" && began.elapsed() < Duration::from_secs(30), "{stopped:?}");
    terminal.dispose().await;
    let _ = std::fs::remove_dir_all(&folder);
}

/// Swift `TerminalPaneLifetimeTests.swift:207 "quitTerminatesEveryTrackedProcessGroupWithinTheBound"`: closing the pane's terminal (the
/// app's quit) ends a polite command and one that ignores SIGTERM, within the bound.
#[tokio::test(flavor = "multi_thread")]
async fn quit_terminates_every_tracked_process_group_within_the_bound() {
    let folder = folder("quit");
    let terminal = terminal(&folder);
    let polite = terminal.start("sleep 30", None).await.unwrap();
    let stubborn = terminal.start("trap '' TERM; echo ar\"\"med; while :; do sleep 1; done", None).await.unwrap();
    until(|| terminal.read_output(&stubborn, 0, 64).output.contains("armed"), 30_000, "the trap is in place").await;
    let began = Instant::now();
    terminal.dispose().await;
    assert!(began.elapsed() < Duration::from_secs(15), "{:?}", began.elapsed());
    assert!(!terminal.is_command_running(&polite) && !terminal.is_command_running(&stubborn));
    assert!(terminal.running_handles().is_empty());
    let _ = std::fs::remove_dir_all(&folder);
}

/// Swift `TerminalPaneLifetimeTests.swift:235 "quitWithNothingRunningReturnsAtOnce"`.
#[tokio::test(flavor = "multi_thread")]
async fn quit_with_nothing_running_returns_at_once() {
    let folder = folder("idle");
    let terminal = terminal(&folder);
    let began = Instant::now();
    terminal.dispose().await;
    assert!(began.elapsed() < Duration::from_millis(500));
    assert!(!terminal.is_running());
    let _ = std::fs::remove_dir_all(&folder);
}

/// Swift `LongRunningProcessTests.swift:101 "realPaneChunksKoreanOutputOnCharacterBoundaries"`: pages of a long Korean output never split
/// a character, and together they are the whole output.
#[tokio::test(flavor = "multi_thread")]
async fn real_pane_chunks_korean_output_on_character_boundaries() {
    let folder = folder("korean");
    let terminal = terminal(&folder);
    let handle = terminal.start("awk 'BEGIN { for (i = 0; i < 15000; i++) printf \"한글\" }'", None).await.unwrap();
    assert_eq!(tokio::time::timeout(Duration::from_secs(30), terminal.exited(&handle)).await.unwrap(), Ok(Some(0)));
    let first = terminal.read_output(&handle, 0, 21_000);
    assert!(first.more_remains && first.output.chars().count() == 21_000 && !first.output.contains('\u{FFFD}'));
    let mut all = first.output.clone();
    let mut offset = first.next_offset;
    loop {
        let page = terminal.read_output(&handle, offset, 21_000);
        assert!(!page.output.contains('\u{FFFD}'));
        all.push_str(&page.output);
        offset = page.next_offset;
        if !page.more_remains {
            break;
        }
    }
    assert!(all.contains(&"한글".repeat(15_000)), "the whole output, in order");
    terminal.dispose().await;
    let _ = std::fs::remove_dir_all(&folder);
}

/// Swift `LongRunningProcessTests.swift:117 "realPaneSkipsACharacterCutByEviction"`: past the 1 MB of bytes the command's log keeps, the
/// oldest output goes, and what is kept never starts inside a character. (The agent's own reads are C#'s buffer, counted in UTF-16
/// units, where these 349,526 characters still fit; Swift counted that one in bytes too and said output was dropped.)
#[tokio::test(flavor = "multi_thread")]
async fn real_pane_skips_a_character_cut_by_eviction() {
    let folder = folder("evict");
    let terminal = terminal(&folder);
    let handle = terminal.start("awk 'BEGIN { for (i = 0; i < 349526; i++) printf \"한\" }'", None).await.unwrap();
    assert_eq!(tokio::time::timeout(Duration::from_secs(60), terminal.exited(&handle)).await.unwrap(), Ok(Some(0)));
    let chunk = terminal.read_output(&handle, 0, 21_000);
    assert!(chunk.next_offset > 21_000 * 3, "the oldest output was dropped: the kept output starts past the beginning");
    assert!(!chunk.output.contains('\u{FFFD}') && chunk.output.starts_with('한'), "{:?}", chunk.output.chars().take(4).collect::<String>());
    terminal.dispose().await;
    let _ = std::fs::remove_dir_all(&folder);
}

/// Swift `LongRunningProcessTests.swift:140 "stopSendsSIGINTAndReturnsAtOnceForACooperativeProcess"`: a command that ends on Ctrl+C is
/// done by it (128 + 2), before the 3 s after which the stop would end its session.
#[tokio::test(flavor = "multi_thread")]
async fn stop_sends_sigint_and_returns_at_once_for_a_cooperative_process() {
    let folder = folder("stop-polite");
    let terminal = terminal(&folder);
    let run = terminal.run("echo ready; exec sleep 30").await.unwrap();
    assert_eq!(run.status, "running");
    until(|| terminal.read_output(&run.handle, 0, 64).output.contains("ready"), 20_000, "the command runs").await;
    let began = Instant::now();
    let stopped = terminal.stop(&run.handle).await.unwrap();
    assert!(stopped.status == "done" && stopped.exit_code == Some(128 + 2), "{stopped:?}");
    assert!(began.elapsed() < Duration::from_secs(25));
    terminal.dispose().await;
    let _ = std::fs::remove_dir_all(&folder);
}

/// Swift `LongRunningProcessTests.swift:187 "realPTYProcessIgnoringSIGINTAndSIGTERMIsStoppedWithinTenSeconds"`, as the Rust stop
/// escalates: a command that ignores SIGINT and SIGTERM is ended with its session 3 s after the Ctrl+C, inside Swift's 10 s bound.
#[tokio::test(flavor = "multi_thread")]
async fn real_pty_process_ignoring_sigint_and_sigterm_is_stopped_within_ten_seconds() {
    let folder = folder("stop-stubborn");
    let terminal = terminal(&folder);
    let run = terminal.run("trap '' INT TERM; echo stubborn; while :; do sleep 1; done").await.unwrap();
    assert_eq!(run.status, "running");
    until(|| terminal.read_output(&run.handle, 0, 64).output.contains("stubborn"), 30_000, "the traps are in place").await;
    let began = Instant::now();
    let stopped = terminal.stop(&run.handle).await.unwrap();
    let took = began.elapsed();
    assert!(stopped.status == "done" && !terminal.is_command_running(&run.handle), "{stopped:?}");
    assert!(took >= Duration::from_millis(2_500) && took < Duration::from_secs(30), "{took:?}");
    terminal.dispose().await;
    let _ = std::fs::remove_dir_all(&folder);
}

/// Swift `AgentTerminalToolTests.swift:34 "realPTYReturnsCombinedOutputAndExitCode"`: the terminal turns each newline into CR LF; the
/// agent reads plain LF lines, standard error among them.
#[tokio::test(flavor = "multi_thread")]
async fn real_pty_returns_combined_output_and_exit_code() {
    let folder = folder("combined");
    let terminal = terminal(&folder);
    let first = terminal.run("echo hi; echo oops >&2; exit 3").await.unwrap();
    let result = finished(&terminal, first).await;
    assert_eq!(result.exit_code, Some(3), "{result:?}");
    assert_eq!(printed(&result), "hi\noops");
    terminal.dispose().await;
    let _ = std::fs::remove_dir_all(&folder);
}

/// Swift `AgentTerminalToolTests.swift:45 "realPTYRunsInTheWorkspaceFolderNotTheAppFolder"`.
#[tokio::test(flavor = "multi_thread")]
async fn real_pty_runs_in_the_workspace_folder_not_the_app_folder() {
    let folder = folder("pwd");
    let terminal = terminal(&folder);
    let first = terminal.run("pwd -P").await.unwrap();
    let result = finished(&terminal, first).await;
    assert_eq!(printed(&result), folder.to_string_lossy());
    assert_ne!(folder, std::env::current_dir().unwrap());
    terminal.dispose().await;
    let _ = std::fs::remove_dir_all(&folder);
}

/// Swift `AgentTerminalToolOutputTests.swift:134 "realPTYColourOutputReachesTheAgentPlainAndThePaneStaysRaw"`: the agent reads the text
/// without its colour, the pane keeps every byte. C#'s cleaner (the agent's reads on both OSes) drops a carriage return rather than
/// resolving the redraw it makes, as Swift's `AgentTerminalOutputCleaner` did (`green\n100%`).
#[tokio::test(flavor = "multi_thread")]
async fn real_pty_colour_output_reaches_the_agent_plain_and_the_pane_stays_raw() {
    let folder = folder("colour");
    let terminal = terminal(&folder);
    let first = terminal.run("printf '\\033[1;32mgreen\\033[0m\\n50%%\\r100%%\\n'").await.unwrap();
    let result = finished(&terminal, first).await;
    assert_eq!(result.exit_code, Some(0), "{result:?}");
    assert_eq!(printed(&result), "green\n50%100%");
    let raw = terminal.read_output(&result.handle, 0, 4_096).output;
    assert!(raw.contains("\u{1b}[1;32mgreen\u{1b}[0m\r\n50%\r100%\r\n"), "{raw:?}");
    terminal.dispose().await;
    let _ = std::fs::remove_dir_all(&folder);
}

/// Swift `LongRunningProcessTests.swift:155 "stopEscalatesToSIGTERMAfterThreeSecondsAndSIGKILLAfterFiveMore"`, as the Rust stop (C#'s)
/// escalates: Ctrl+C first, and 3 s later the command's whole session ended at once (Swift: SIGTERM at 3 s, SIGKILL 5 s later); the
/// stop reports the command done, killed, within Swift's 10 s.
#[tokio::test(flavor = "multi_thread")]
async fn stop_escalates_to_sigterm_after_three_seconds_and_sigkill_after_five_more() {
    let folder = folder("stop-escalates");
    let terminal = terminal(&folder);
    let run = terminal.run("trap '' INT; echo armed; while :; do sleep 1; done").await.unwrap();
    until(|| terminal.read_output(&run.handle, 0, 64).output.contains("armed"), 30_000, "the trap is in place").await;
    let began = Instant::now();
    let stopped = terminal.stop(&run.handle).await.unwrap();
    let took = began.elapsed();
    assert!(stopped.status == "done" && stopped.exit_code == Some(128 + 9), "ended by SIGKILL: {stopped:?}");
    assert!(took >= Duration::from_millis(2_500) && took <= Duration::from_secs(10), "{took:?}");
    terminal.dispose().await;
    let _ = std::fs::remove_dir_all(&folder);
}

/// Swift `LongRunningProcessTests.swift:222 "realPTYCooperativeProcessStopsOnSIGINTQuickly"`: the real terminal delivers the stop's
/// Ctrl+C and the command ends by SIGINT; the bound only catches a stop that hangs.
#[tokio::test(flavor = "multi_thread")]
async fn real_pty_cooperative_process_stops_on_sigint_quickly() {
    let folder = folder("stop-quick");
    let terminal = terminal(&folder);
    let run = terminal.run("sleep 30").await.unwrap();
    assert_eq!(run.status, "running");
    tokio::time::sleep(Duration::from_millis(300)).await;
    let began = Instant::now();
    let stopped = terminal.stop(&run.handle).await.unwrap();
    assert!(stopped.status == "done", "{stopped:?}");
    assert!(began.elapsed() < Duration::from_secs(25));
    terminal.dispose().await;
    let _ = std::fs::remove_dir_all(&folder);
}
