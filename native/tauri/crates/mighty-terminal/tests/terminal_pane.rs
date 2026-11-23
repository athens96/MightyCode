//! A terminal pane as the page drives it (C# MainWindow.Terminal.cs and the terminal half of MainWindow.WebShell.Panes.cs, which
//! C#'s web smoke exercises; there is no Core check of the WinUI pane): the footer, the shell started by the page's `ready`, its
//! output flushed as base64 UTF-8, its input, size, title, directory, the clipboard's limits, the exit and the restart, the shell's
//! activity, an agent's terminal pane, and the teardown. Against the fake terminal (a loopback).

mod common;

use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use common::*;
use mighty_platform::Clipboard;
use mighty_platform::fake::KILLED;
use mighty_terminal::pane::{COPY_LIMIT, PASTE_LIMIT, directory_from_url};
use mighty_terminal::signals::Signal;
use mighty_terminal::{AgentTerminal, PaneEvents, PaneKind, PaneOptions, TerminalFont, TerminalMessage, TerminalPane};
use serde_json::json;

#[tokio::test(flavor = "multi_thread")]
async fn a_shell_pane_starts_with_the_pages_terminal_and_echoes_its_input() {
    let fixture = Fixture::new("pane");
    let pane = fixture.pane(PaneKind::Shell, Some(fixture.folder.clone()));
    let footer = pane.open();
    assert!(footer.available && footer.shell && footer.restart && !footer.restart_enabled, "{footer:?}");
    assert_eq!(footer.notice.as_deref(), Some("Starting terminal…"));
    assert_eq!((footer.engine.as_str(), footer.title.as_str(), footer.grid.as_str()), ("ConPTY", "shell", ""));
    assert_eq!(footer.directory, fixture.folder.display().to_string());
    let json = serde_json::to_value(TerminalMessage::Footer { footer: footer.clone() }).unwrap();
    assert_eq!(json["type"], "footer");
    assert_eq!(json["footer"]["restartEnabled"], false, "the page's field names: {json}");

    pane.handle_message(&json!({"type": "ready", "columns": 80, "rows": 24})).await;
    let opened = fixture.ptys.opened();
    assert_eq!(opened.len(), 1, "the page's ready starts the shell");
    assert_eq!((opened[0].size.columns, opened[0].size.rows), (80, 24), "at the page's grid");
    assert_eq!(opened[0].cwd, fixture.folder, "in the workspace's folder");
    assert_eq!(opened[0].env.get("PATH").map(String::as_str), Some("/fake/bin"), "with the CLI environment");
    assert_eq!(fixture.window.statuses(), ["idle"], "attached: idle until a reading says a command runs");
    assert!(
        fixture.window.posts().contains(&TerminalMessage::Theme { light: false })
            && fixture.window.posts().contains(&TerminalMessage::Font { size: 12.0, family: None }),
        "the colours and the font follow the start"
    );
    let running = pane.footer();
    assert!(running.notice.is_none() && !running.restart && running.grid == "80×24", "a running terminal has no notice: {running:?}");
    pane.tick();
    assert!(
        fixture.window.posts().iter().any(|m| matches!(m, TerminalMessage::Footer { footer } if footer == &running)),
        "the changed footer is pushed"
    );
    let footers = |window: &Window| window.posts().iter().filter(|m| matches!(m, TerminalMessage::Footer { .. })).count();
    let before = footers(&fixture.window);
    pane.tick();
    assert_eq!(footers(&fixture.window), before, "an unchanged footer is not pushed again");

    pane.handle_message(&json!({"type": "input", "text": "echo 한글\r"})).await;
    until(
        || {
            pane.tick();
            fixture.window.output().contains("echo 한글\r")
        },
        5_000,
        "the input echoed back as output",
    )
    .await;
    assert_eq!(pane.diagnostics()["inputs"], 1);

    pane.handle_message(&json!({"type": "resize", "columns": 100, "rows": 40})).await;
    assert!(fixture.ptys.sizes().iter().any(|s| s.columns == 100 && s.rows == 40) && pane.footer().grid == "100×40", "resized");
    pane.handle_message(&json!({"type": "resize", "columns": 5000, "rows": 0})).await;
    assert_eq!(pane.footer().grid, "1000×1", "the grid is clamped as the console clamps it");
    pane.handle_message(&json!({"type": "resize", "columns": "wide"})).await;
    assert_eq!(fixture.window.errors().len(), 1, "a size that is no number is reported");

    pane.handle_message(&json!({"type": "title", "title": "pwsh"})).await;
    pane.handle_message(&json!({"type": "title", "title": "a\u{7}b"})).await;
    pane.handle_message(&json!({"type": "title", "title": "x".repeat(201)})).await;
    assert_eq!(pane.footer().title, "pwsh", "a title with a control character or over 200 units is not taken");

    let (url, path) = if cfg!(windows) { ("file:///C:/work/x%20y", "C:\\work\\x y") } else { ("file:///tmp/x%20y", "/tmp/x y") };
    pane.handle_message(&json!({"type": "directory", "url": url})).await;
    assert_eq!(pane.footer().directory, path);
    pane.handle_message(&json!({"type": "directory", "url": "file://server/share/x"})).await;
    pane.handle_message(&json!({"type": "directory", "url": "https://example.com/x"})).await;
    assert_eq!(pane.footer().directory, path, "a share or a web address is no working directory");

    pane.handle_message(&json!({"type": "pressed"})).await;
    assert_eq!(*fixture.window.pressed.lock().unwrap(), ["pane"], "a press makes the pane the active one");

    // MC-26: the terminal's notification and bell reach the window, the words made safe.
    pane.handle_message(&json!({"type": "notify", "title": "Build", "body": "done\u{1b}[31m\n ok"})).await;
    pane.handle_message(&json!({"type": "bell"})).await;
    assert_eq!(
        *fixture.window.signals.lock().unwrap(),
        [("pane".to_string(), Signal::Notify { title: "Build".into(), body: "done [31m ok".into() }), ("pane".to_string(), Signal::Bell)]
    );

    pane.handle_message(&json!({"type": "copy", "text": "selected"})).await;
    assert_eq!(fixture.clipboard.text().as_deref(), Some("selected"), "copied");
    pane.handle_message(&json!({"type": "copy", "text": "x".repeat(COPY_LIMIT + 1)})).await;
    assert_eq!(fixture.clipboard.text().as_deref(), Some("selected"), "a selection over 1,000,000 units is not copied");
    pane.handle_message(&json!({"type": "paste"})).await;
    assert!(fixture.window.posts().contains(&TerminalMessage::Paste { text: "selected".into() }), "pasted");
    fixture.clipboard.write_text(&"x".repeat(PASTE_LIMIT + 1)).unwrap();
    pane.handle_message(&json!({"type": "paste"})).await;
    assert_eq!(fixture.window.errors().last().map(String::as_str), Some("Paste up to 65,536 characters at a time."));
    fixture.clipboard.set_busy(true);
    pane.handle_message(&json!({"type": "copy", "text": "x"})).await;
    assert!(fixture.window.errors().last().is_some_and(|e| e.contains("in use")), "a clipboard that refused is reported");
    fixture.clipboard.set_busy(false);

    // The shell exits by itself: completed, the notice with its code and the restart button.
    fixture.ptys.exit(0, 7);
    until(|| fixture.window.statuses().last() == Some(&"completed"), 5_000, "the exit").await;
    let exited = pane.footer();
    assert!(exited.notice.as_deref() == Some("Shell exited (7).") && exited.restart && exited.restart_enabled, "{exited:?}");
    pane.handle_message(&json!({"type": "input", "text": "ls\r"})).await;
    assert_eq!(
        fixture.window.errors().last().map(String::as_str),
        Some("Terminal input is full or the shell has exited. Try again."),
        "a shell that exited takes no input"
    );
    assert!(!pane.read_activity(), "an exited shell is no longer read");

    pane.restart().await;
    assert_eq!(fixture.ptys.opened().len(), 2, "a new shell");
    assert!(fixture.window.posts().contains(&TerminalMessage::Reset), "the page's terminal is reset first");
    assert_eq!(&fixture.window.statuses()[fixture.window.statuses().len() - 2..], ["idle", "idle"], "idle at once, idle once attached");
    let restarted = pane.footer();
    assert!(restarted.notice.is_none() && !restarted.restart, "{restarted:?}");

    pane.dispose().await;
    assert!(pane.is_closed() && fixture.ptys.closed(1), "the pane's shell is closed with it");
    let count = fixture.window.posts().len();
    pane.tick();
    pane.handle_message(&json!({"type": "input", "text": "after\r"})).await;
    assert_eq!(fixture.window.posts().len(), count, "a closed pane says nothing more");
    pane.dispose().await;
}

#[tokio::test]
async fn a_pane_without_a_terminal_here() {
    let fixture = Fixture::new("remote");
    let pane = fixture.pane(PaneKind::Shell, None);
    let footer = pane.open();
    assert!(!footer.available && footer.notice.is_none() && !footer.restart && footer.title.is_empty(), "{footer:?}");
    pane.handle_message(&json!({"type": "ready", "columns": 80, "rows": 24})).await;
    assert!(fixture.ptys.opened().is_empty(), "a remote workspace's shell is its host's");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_shell_that_does_not_start_says_why() {
    let fixture = Fixture::new("refused");
    let pane = fixture.pane(PaneKind::Shell, Some(fixture.folder.clone()));
    fixture.ptys.fail_next_opens(1);
    pane.handle_message(&json!({"type": "ready", "columns": 80, "rows": 24})).await;
    assert_eq!(fixture.window.errors(), ["the fake terminal refused to start"]);
    let footer = pane.footer();
    assert_eq!(footer.notice.as_deref(), Some("Could not start the terminal.\nthe fake terminal refused to start"));
    assert!(footer.restart && footer.restart_enabled, "the restart button is offered");
    assert_eq!(fixture.window.statuses(), ["error"]);
    pane.restart().await;
    assert!(pane.is_running() && fixture.window.statuses().last() == Some(&"idle"), "the restart started it");
    pane.dispose().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn the_shells_activity_moves_the_pane() {
    let fixture = Fixture::new("activity");
    let pane = fixture.pane(PaneKind::Shell, Some(fixture.folder.clone()));
    assert!(!pane.read_activity(), "no shell yet");
    pane.handle_message(&json!({"type": "ready", "columns": 80, "rows": 24})).await;
    fixture.ptys.set_busy(0, Some(true));
    assert!(pane.read_activity());
    assert_eq!(fixture.window.statuses(), ["idle", "running"], "a command on the terminal");
    assert!(pane.read_activity());
    assert_eq!(fixture.window.statuses().len(), 2, "only a different status is reported");
    fixture.ptys.set_busy(0, None);
    pane.read_activity();
    assert_eq!(fixture.window.statuses().len(), 2, "a reading that could not be taken changes nothing");
    fixture.ptys.set_busy(0, Some(false));
    pane.read_activity();
    assert_eq!(fixture.window.statuses(), ["idle", "running", "idle"], "back at the prompt");
    pane.dispose().await;
    assert!(!pane.read_activity(), "a closed pane is not read");
}

#[tokio::test(flavor = "multi_thread")]
async fn output_waits_for_a_ready_page_and_a_second_ready_takes_the_look_again() {
    let fixture = Fixture::new("ready-again");
    let pane = fixture.pane(PaneKind::Shell, Some(fixture.folder.clone()));
    pane.tick();
    assert!(fixture.window.posts().is_empty(), "nothing before the page opens the pane");
    pane.handle_message(&json!({"type": "ready", "columns": 80, "rows": 24})).await;
    *fixture.window.light.lock().unwrap() = true;
    pane.handle_message(&json!({"type": "ready", "columns": 80, "rows": 24})).await;
    assert_eq!(fixture.ptys.opened().len(), 1, "a page loaded again starts no second shell");
    assert!(fixture.window.posts().contains(&TerminalMessage::Theme { light: true }), "its new terminal takes the colours again");
    let message = json!({"type": "input", "text": "x".repeat(1_048_577)});
    pane.handle_message(&message).await;
    assert_eq!(pane.diagnostics()["inputs"], 0, "a message over 1 MiB is not read");
    pane.dispose().await;
}

/// C# `RefreshTerminalTheme` on a theme or text setting change: a ready terminal takes the colours and the font again; one the page
/// has not opened yet takes them when it is ready.
#[tokio::test(flavor = "multi_thread")]
async fn a_new_theme_or_text_setting_reaches_a_ready_terminal() {
    let fixture = Fixture::new("restyle");
    let pane = fixture.pane(PaneKind::Shell, Some(fixture.folder.clone()));
    pane.restyle();
    assert!(fixture.window.posts().is_empty(), "nothing before the page opens the pane");
    pane.handle_message(&json!({"type": "ready", "columns": 80, "rows": 24})).await;
    *fixture.window.light.lock().unwrap() = true;
    *fixture.window.font.lock().unwrap() = TerminalFont { size: 18.0, family: Some("Fira Code".into()) };
    pane.restyle();
    let posts = fixture.window.posts();
    assert!(posts.contains(&TerminalMessage::Theme { light: true }));
    assert!(posts.contains(&TerminalMessage::Font { size: 18.0, family: Some("Fira Code".into()) }));
    pane.dispose().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn an_agent_terminal_pane_shows_its_owners_commands() {
    let fixture = Fixture::new("agent-pane");
    let missing = fixture.pane(PaneKind::Agent { owner: Some("agent".into()) }, Some(fixture.folder.clone()));
    let footer = missing.open();
    assert!(footer.available && !footer.shell && !footer.restart && footer.engine == "Terminal", "{footer:?}");
    missing.handle_message(&json!({"type": "ready", "columns": 80, "rows": 24})).await;
    assert_eq!(fixture.window.errors(), ["Agent terminal is no longer available."]);
    assert_eq!(missing.footer().notice.as_deref(), Some("Could not start the terminal.\nAgent terminal is no longer available."));
    missing.dispose().await;

    let agent = fixture.agent();
    fixture.window.agents.lock().unwrap().insert("agent".into(), agent.clone());
    let handle = agent.start("npm run build", None).await.unwrap();
    let pane = fixture.pane(PaneKind::Agent { owner: Some("agent".into()) }, Some(fixture.folder.clone()));
    pane.handle_message(&json!({"type": "ready", "columns": 90, "rows": 20})).await;
    assert!(pane.footer().notice.is_none(), "connected");
    assert!(fixture.ptys.sizes().iter().any(|s| s.columns == 90 && s.rows == 20), "the command's terminal takes the pane's size");
    pane.tick();
    assert!(fixture.window.output().contains("> npm run build"), "what was printed before shows first");
    pane.handle_message(&json!({"type": "input", "text": "yes\r"})).await;
    until(
        || {
            pane.tick();
            fixture.window.output().contains("yes\r")
        },
        5_000,
        "the input reached the newest command",
    )
    .await;
    pane.dispose().await;
    assert!(agent.is_command_running(&handle), "closing the pane leaves the agent's command running");
    let stopped = tokio::time::timeout(Duration::from_secs(10), agent.stop(&handle)).await.unwrap().unwrap();
    assert_eq!((stopped.status, stopped.exit_code), ("done", Some(KILLED)));
    agent.dispose().await;
}

/// A window whose next output post is held (a page slow to take it) until the check lets it go; a post is recorded once it ends.
struct Held {
    window: Arc<Window>,
    hold: Mutex<Option<(mpsc::Sender<()>, mpsc::Receiver<()>)>>,
}

impl PaneEvents for Held {
    fn post(&self, pane: &str, message: &TerminalMessage) {
        let held = if matches!(message, TerminalMessage::Output { .. }) { self.hold.lock().unwrap().take() } else { None };
        if let Some((entered, release)) = held {
            entered.send(()).unwrap();
            release.recv().unwrap();
        }
        self.window.post(pane, message);
    }
    fn status(&self, pane: &str, status: &'static str) {
        self.window.status(pane, status);
    }
    fn activity(&self, pane: &str, busy: bool) {
        self.window.activity(pane, busy);
    }
    fn error(&self, text: String) {
        self.window.error(text);
    }
    fn pressed(&self, pane: &str) {
        self.window.pressed(pane);
    }
    fn light(&self) -> bool {
        self.window.light()
    }
    fn font(&self) -> TerminalFont {
        self.window.font()
    }
    fn agent_terminal(&self, owner: &str) -> Option<Arc<AgentTerminal>> {
        self.window.agent_terminal(owner)
    }
}

/// Output a flush took before a restart reaches the page before the restart's reset, never after it (where it would draw the old
/// shell's output into the new one's terminal).
#[tokio::test(flavor = "multi_thread")]
async fn output_taken_before_a_restart_never_follows_its_reset() {
    let fixture = Fixture::new("outbox");
    let (entered_sender, entered) = mpsc::channel();
    let (release, release_receiver) = mpsc::channel();
    let held = Arc::new(Held { window: fixture.window.clone(), hold: Mutex::new(None) });
    let options = PaneOptions {
        id: "pane".into(),
        kind: PaneKind::Shell,
        folder: Some(fixture.folder.clone()),
        enabled: true,
        no_profile: true,
        remote: None,
    };
    let pane = TerminalPane::new(options, fixture.ports(), held.clone());
    pane.handle_message(&json!({"type": "ready", "columns": 80, "rows": 24})).await;
    pane.handle_message(&json!({"type": "input", "text": "old shell output"})).await;
    until(|| pane.diagnostics()["pending"].as_u64().unwrap_or(0) > 0, 5_000, "the old shell's output waiting").await;
    *held.hold.lock().unwrap() = Some((entered_sender, release_receiver));
    let flushing = pane.clone();
    let flush = std::thread::spawn(move || flushing.flush());
    entered.recv_timeout(Duration::from_secs(5)).expect("the flush posts the output");
    let restarting = pane.clone();
    let restart = tokio::spawn(async move { restarting.restart().await });
    // The restart has closed the old shell and wants to reset the page meanwhile.
    tokio::time::sleep(Duration::from_millis(300)).await;
    release.send(()).unwrap();
    flush.join().unwrap();
    tokio::time::timeout(Duration::from_secs(10), restart).await.expect("the restart ends").unwrap();
    let posts = fixture.window.posts();
    let output = posts.iter().position(|m| matches!(m, TerminalMessage::Output { .. })).expect("the output was posted");
    let reset = posts.iter().position(|m| *m == TerminalMessage::Reset).expect("the reset was posted");
    assert!(output < reset, "the old output first, then the reset that clears it: {posts:?}");
    pane.dispose().await;
}

#[test]
fn a_working_directory_is_a_file_address_of_this_computer() {
    assert_eq!(directory_from_url("file://server/share"), None);
    assert_eq!(directory_from_url("ftp://x/y"), None);
    assert_eq!(directory_from_url(&format!("file:///{}", "a".repeat(8200))), None, "over 8,192 units");
    if cfg!(windows) {
        assert_eq!(directory_from_url("file://localhost/C:/Users").as_deref(), Some("C:\\Users"));
        assert_eq!(directory_from_url("file:///D:/w/%ED%95%9C").as_deref(), Some("D:\\w\\한"));
    }
    assert_eq!(TerminalFont::default(), TerminalFont { size: 12.0, family: None });
}

/// A PTY host whose opens wait until the check lets them go (the Mac's shell takes a moment to start).
struct SlowOpen {
    inner: Arc<mighty_platform::fake::FakePtyHost>,
    release: Arc<tokio::sync::Notify>,
}

impl mighty_platform::PtyHost for SlowOpen {
    fn default_shell(&self) -> std::path::PathBuf {
        self.inner.default_shell()
    }
    fn shell_arguments(&self, shell: &std::path::Path, command: Option<&str>, no_profile: bool) -> Vec<String> {
        self.inner.shell_arguments(shell, command, no_profile)
    }
    fn open(
        &self,
        request: mighty_platform::PtyRequest,
    ) -> mighty_platform::BoxFuture<'_, mighty_platform::Result<mighty_platform::PtySession>> {
        Box::pin(async move {
            self.release.notified().await;
            self.inner.open(request).await
        })
    }
}

/// Mac stage integration (the Mac smoke's `terminalTty`): the view's grid settles while the shell is still starting; that resize has
/// no terminal yet, so the terminal is set to it once it opens, and the shell sees the view's grid.
#[tokio::test(flavor = "multi_thread")]
async fn a_resize_while_the_shell_starts_reaches_its_terminal() {
    let fixture = Fixture::new("slow-open");
    let release = Arc::new(tokio::sync::Notify::new());
    let mut ports = fixture.ports();
    ports.ptys = Arc::new(SlowOpen { inner: fixture.ptys.clone(), release: release.clone() });
    let options = PaneOptions {
        id: "pane".into(),
        kind: PaneKind::Shell,
        folder: Some(fixture.folder.clone()),
        enabled: true,
        no_profile: true,
        remote: None,
    };
    let pane = TerminalPane::new(options, ports, fixture.window.clone());
    pane.open();
    let starting = {
        let pane = pane.clone();
        tokio::spawn(async move { pane.handle_message(&json!({"type": "ready", "columns": 106, "rows": 27})).await })
    };
    tokio::time::sleep(Duration::from_millis(100)).await;
    pane.handle_message(&json!({"type": "resize", "columns": 106, "rows": 29})).await;
    release.notify_one();
    starting.await.unwrap();
    let opened = fixture.ptys.opened();
    assert_eq!((opened.len(), opened[0].size.rows), (1, 27), "opened at the first grid");
    assert_eq!(fixture.ptys.sizes().last().map(|s| (s.columns, s.rows)), Some((106, 29)), "then set to the grid the view settled on");
    assert_eq!(pane.footer().grid, "106×29");
}

/// MC-33: an SSH pane's terminal runs the host's ssh instead of the local shell, offers the dev-server ports its output names, and a
/// lost connection (ssh's 255) reads as such with the restart button turned into 다시 연결.
#[tokio::test(flavor = "multi_thread")]
async fn an_ssh_pane_runs_ssh_and_says_when_the_connection_went() {
    let fixture = Fixture::new("ssh-pane");
    let remote = mighty_terminal::RemoteShell {
        program: "/usr/bin/ssh".into(),
        args: vec!["-tt".into(), "--".into(), "devbox".into(), "cd '/srv/app' && exec $SHELL -l".into()],
        title: "ssh · devbox".into(),
        directory: "devbox:/srv/app".into(),
    };
    let options = PaneOptions {
        id: "pane".into(),
        kind: PaneKind::Shell,
        folder: Some(fixture.folder.clone()),
        enabled: true,
        no_profile: true,
        remote: Some(remote.clone()),
    };
    let pane = TerminalPane::new(options, fixture.ports(), fixture.window.clone());
    assert_eq!(pane.footer().title, "ssh · devbox");
    assert_eq!(pane.footer().directory, "devbox:/srv/app");
    pane.handle_message(&json!({"type": "ready", "columns": 80, "rows": 24})).await;
    until(|| !fixture.ptys.opened().is_empty(), 5_000, "the terminal opens").await;
    let opened = fixture.ptys.opened()[0].clone();
    assert_eq!(opened.program, remote.program, "the local ssh runs, not the local shell");
    assert_eq!(opened.args, remote.args);
    assert_eq!(opened.cwd, fixture.folder, "in the workspace's stand-in folder");

    pane.handle_message(&json!({"type": "input", "text": "  Local:   http://localhost:\u{1b}[1m5173\u{1b}[22m/"})).await;
    until(
        || {
            pane.flush();
            pane.seen_ports() == [5173]
        },
        5_000,
        "the dev server's port is read through its colours",
    )
    .await;

    fixture.ptys.exit(0, 255);
    until(|| pane.lost(), 5_000, "the lost connection").await;
    let footer = pane.footer();
    assert!(footer.reconnect && footer.restart, "the restart button reads 다시 연결");
    assert_eq!(footer.notice.as_deref(), Some(mighty_core::locale::Locale::new("en").get("ssh.terminal.lost").as_str()));
    pane.dispose().await;
}

/// A local shell's exit code 255 is only an exit code.
#[tokio::test(flavor = "multi_thread")]
async fn a_local_shells_255_is_no_lost_connection() {
    let fixture = Fixture::new("local-255");
    let pane = fixture.pane(PaneKind::Shell, Some(fixture.folder.clone()));
    pane.handle_message(&json!({"type": "ready", "columns": 80, "rows": 24})).await;
    until(|| !fixture.ptys.opened().is_empty(), 5_000, "the terminal opens").await;
    fixture.ptys.exit(0, 255);
    until(|| pane.footer().restart_enabled, 5_000, "the exit").await;
    assert!(!pane.lost() && !pane.footer().reconnect);
    pane.dispose().await;
}
