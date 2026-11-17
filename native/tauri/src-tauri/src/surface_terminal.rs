//! M-D: a terminal pane's native view (the Mac's Ghostty, `mighty-platform-macos` `terminal_surface`), fed by the pane's PTY, which
//! the host owns on both OSes (Mac plan D2). Where the page draws terminals (Windows: xterm.js) none of this runs: `SurfaceManager`
//! never asks for a terminal view there, so no view is ever registered and [`post`] hands every message back to the page.
//!
//! The native view stands where the page's xterm.js stands on Windows, so mighty-terminal's pane logic (the shell's start at the
//! view's grid, the output kept while nothing takes it, the restart, the exit notice, the shell's activity, the footer, an agent pane's
//! commands) runs unchanged:
//! - [`open`]: the view for a terminal pane's slot, the first time the slot can show (surface.rs calls it); its
//!   [`TerminalEvent`]s become the pane's `terminalMessage`s ([`message`]), handled in order: the first grid is `ready` (the shell
//!   starts at it), each later one a `resize`, what the user typed `input`, the title `title`, the working folder `directory`, the
//!   view taking the keyboard `pressed` (its pane becomes the active one);
//! - [`post`]: what the pane would push to the page's terminal goes to its view instead (terminal.rs' `HostEvents::post`, "the Mac
//!   feed"): the output fed to Ghostty, a restart's reset, the theme and the font; the footer still goes to the page, which draws it
//!   under the slot.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};

use base64::Engine;
use mighty_platform::{BoxFuture, DeviceRect, SurfaceFactory, TerminalEvent, TerminalListener, TerminalOptions, TerminalSurface};
use mighty_terminal::TerminalMessage;
use serde_json::{Value, json};

/// Where a view's messages go: its pane's `terminalMessage` (terminal.rs connects it when the terminals start). Each is awaited
/// before the next is handed over, so the pane takes them in the order the view sent them.
pub type ToPane = Arc<dyn Fn(String, Value) -> BoxFuture<'static, ()> + Send + Sync>;

/// The most text one `input` message carries (characters): a long paste goes as several, each well inside what a PTY write takes
/// (`PtyControl::write`, 65,536 UTF-16 units).
const INPUT_CHUNK: usize = 8192;

/// ESC c (RIS): the terminal's full reset, what a restart's `reset` does to the page's xterm.
const RESET: &[u8] = b"\x1bc";

/// The terminal panes' native views, by pane.
#[derive(Default)]
pub struct NativeTerminals {
    views: Mutex<HashMap<String, Arc<dyn TerminalSurface>>>,
    /// The panes whose view could not open: not tried again until the pane is forgotten (the platform said why once).
    failed: Mutex<HashSet<String>>,
    to_pane: Mutex<Option<ToPane>>,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

impl NativeTerminals {
    /// Where the views' messages go from now on.
    pub fn connect(&self, to_pane: ToPane) {
        *lock(&self.to_pane) = Some(to_pane);
    }

    /// Opens `pane`'s view at `rect` (it opens shown) and registers it; None when the terminals have not started or the platform gave
    /// no view. The view's events wait in order until it is registered, so the pane's first answer (the theme, the font) finds it.
    pub async fn open(&self, factory: &Arc<dyn SurfaceFactory>, pane: &str, rect: DeviceRect) -> Option<Arc<dyn TerminalSurface>> {
        let to_pane = lock(&self.to_pane).clone()?;
        if lock(&self.failed).contains(pane) {
            return None;
        }
        let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel::<Value>();
        let ready = AtomicBool::new(false);
        let listener = TerminalListener::new(move |event| {
            for message in message(&ready, event) {
                let _ = sender.send(message);
            }
        });
        let options = TerminalOptions {
            pane_id: pane.to_string(),
            cwd: PathBuf::new(),
            program: None,
            args: Vec::new(),
            env: BTreeMap::new(),
            bounds: rect,
            listener: Some(listener),
        };
        let view: Arc<dyn TerminalSurface> = match factory.open_terminal(options).await {
            Ok(view) => Arc::from(view),
            Err(_) => {
                lock(&self.failed).insert(pane.to_string());
                return None;
            }
        };
        lock(&self.views).insert(pane.to_string(), view.clone());
        let owner = pane.to_string();
        tokio::spawn(async move {
            while let Some(message) = receiver.recv().await {
                to_pane(owner.clone(), message).await;
            }
        });
        Some(view)
    }

    /// The pane's view, while it has one.
    pub fn view(&self, pane: &str) -> Option<Arc<dyn TerminalSurface>> {
        lock(&self.views).get(pane).cloned()
    }

    /// The pane is gone: its view no longer takes what the pane sends (surface.rs closes the view itself).
    pub fn forget(&self, pane: &str) {
        lock(&self.views).remove(pane);
        lock(&self.failed).remove(pane);
    }

    /// A message the pane would push to the page's terminal, given to its view instead; false when the pane has no view (or the
    /// message is the footer, which the page draws), so it goes to the page as on Windows. The output waits while the view's write
    /// queue is full (backpressure: the pane's flush, and so its pending output, waits with it).
    pub fn post(&self, pane: &str, message: &TerminalMessage) -> bool {
        if matches!(message, TerminalMessage::Footer { .. }) {
            return false;
        }
        let Some(view) = self.view(pane) else { return false };
        // A view that closed meanwhile refuses: the pane is going too.
        let _ = match message {
            TerminalMessage::Output { data } => match base64::engine::general_purpose::STANDARD.decode(data) {
                Ok(bytes) => view.feed(&bytes),
                Err(_) => Ok(()),
            },
            TerminalMessage::Reset => view.feed(RESET),
            TerminalMessage::Focus => view.focus(),
            TerminalMessage::Font { size, family } => view.set_font(family.as_deref().unwrap_or(""), *size),
            TerminalMessage::Theme { light } => view.set_theme(*light),
            // The answer to the page's xterm asking for the clipboard: a native view pastes the clipboard itself.
            TerminalMessage::Paste { .. } => Ok(()),
            TerminalMessage::Footer { .. } => Ok(()),
        };
        true
    }
}

/// The `terminalMessage`s one view event is, as the page's xterm would have sent them (none for the bell or a lost keyboard):
/// `ready` once (`ready` turns true), `resize` after, the input in chunks of at most [`INPUT_CHUNK`] characters (bytes that are not
/// UTF-8 as `inputBytes`, base64, so they reach the PTY as they were).
pub fn message(ready: &AtomicBool, event: TerminalEvent) -> Vec<Value> {
    match event {
        TerminalEvent::Resized { columns, rows, .. } => {
            let kind = if ready.swap(true, Ordering::SeqCst) { "resize" } else { "ready" };
            vec![json!({"type": kind, "columns": columns, "rows": rows})]
        }
        TerminalEvent::Input(bytes) => {
            // Input that is not UTF-8 (an X10 mouse report) goes as it is, never through a lossy decoding.
            let Ok(text) = std::str::from_utf8(&bytes) else {
                return bytes
                    .chunks(INPUT_CHUNK)
                    .map(|chunk| json!({"type": "inputBytes", "data": base64::engine::general_purpose::STANDARD.encode(chunk)}))
                    .collect();
            };
            let mut messages = Vec::new();
            let mut start = 0;
            for (count, (at, _)) in text.char_indices().enumerate() {
                if count > 0 && count % INPUT_CHUNK == 0 {
                    messages.push(json!({"type": "input", "text": &text[start..at]}));
                    start = at;
                }
            }
            if start < text.len() {
                messages.push(json!({"type": "input", "text": &text[start..]}));
            }
            messages
        }
        TerminalEvent::Title(title) => vec![json!({"type": "title", "title": title})],
        TerminalEvent::WorkingDirectory(folder) => {
            directory_url(&folder).map(|url| json!({"type": "directory", "url": url})).into_iter().collect()
        }
        TerminalEvent::Focused(true) => vec![json!({"type": "pressed"})],
        TerminalEvent::Focused(false) | TerminalEvent::Bell => Vec::new(),
    }
}

/// The working folder the terminal reported (Ghostty gives the path it read from the shell's OSC 7), as the `file:` address the pane
/// checks (`directory_from_url`); an address is kept as it is.
fn directory_url(folder: &str) -> Option<String> {
    if folder.starts_with("file:") {
        return Some(folder.to_string());
    }
    tauri::Url::from_file_path(folder).ok().map(String::from)
}

/// The app's terminal views (one window).
pub fn natives() -> &'static NativeTerminals {
    static NATIVES: OnceLock<NativeTerminals> = OnceLock::new();
    NATIVES.get_or_init(NativeTerminals::default)
}

/// Opens `pane`'s Ghostty view at `rect` over the page, its events wired to its pane (surface.rs, the first time the slot can show).
pub async fn open(factory: &Arc<dyn SurfaceFactory>, pane: &str, rect: DeviceRect) -> Option<Arc<dyn TerminalSurface>> {
    natives().open(factory, pane, rect).await
}

/// What `pane` would push to the page's terminal, to its native view when it has one (terminal.rs' Mac feed).
pub fn post(pane: &str, message: &TerminalMessage) -> bool {
    natives().post(pane, message)
}

#[cfg(test)]
mod tests {
    use super::*;
    use mighty_platform::fake::{FakeSurfaces, SurfaceEvent};
    use mighty_platform::pty::TerminalInput;

    const RECT: DeviceRect = DeviceRect { x: 10, y: 20, width: 600, height: 400 };

    /// The messages the views sent, with their panes.
    type Sent = Arc<Mutex<Vec<(String, Value)>>>;

    /// A registry whose messages are recorded, and a native-terminal factory.
    fn natives_with_log() -> (NativeTerminals, Arc<FakeSurfaces>, Sent) {
        let natives = NativeTerminals::default();
        let log: Sent = Arc::default();
        let sink = log.clone();
        natives.connect(Arc::new(move |pane, message| {
            let sink = sink.clone();
            Box::pin(async move { lock(&sink).push((pane, message)) })
        }));
        (natives, Arc::new(FakeSurfaces::new(true)), log)
    }

    async fn until(test: impl Fn() -> bool) {
        for _ in 0..200 {
            if test() {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    }

    /// The view's grid is the pane's `ready` once, then `resize`; typing is `input`; a title and a working folder are the page's own
    /// messages; the view taking the keyboard is a press in the pane; the bell and a lost keyboard say nothing.
    #[test]
    fn a_views_events_are_the_pages_terminal_messages() {
        let ready = AtomicBool::new(false);
        let grid = |columns, rows| TerminalEvent::Resized { columns, rows, width: 800, height: 600 };
        assert_eq!(message(&ready, grid(97, 32)), [json!({"type": "ready", "columns": 97, "rows": 32})]);
        assert_eq!(message(&ready, grid(80, 24)), [json!({"type": "resize", "columns": 80, "rows": 24})]);
        assert_eq!(message(&ready, TerminalEvent::Input(b"ls\r".to_vec())), [json!({"type": "input", "text": "ls\r"})]);
        assert_eq!(message(&ready, TerminalEvent::Title("zsh".into())), [json!({"type": "title", "title": "zsh"})]);
        let folder = if cfg!(windows) { r"C:\work\nested" } else { "/work/nested" };
        let url = tauri::Url::from_file_path(folder).unwrap().to_string();
        assert_eq!(message(&ready, TerminalEvent::WorkingDirectory(folder.into())), [json!({"type": "directory", "url": url})]);
        assert_eq!(
            message(&ready, TerminalEvent::WorkingDirectory("file:///work/nested".into())),
            [json!({"type": "directory", "url": "file:///work/nested"})],
            "an address is kept as the terminal gave it"
        );
        assert!(message(&ready, TerminalEvent::WorkingDirectory("relative/folder".into())).is_empty(), "no folder of this computer");
        assert_eq!(message(&ready, TerminalEvent::Focused(true)), [json!({"type": "pressed"})]);
        assert!(message(&ready, TerminalEvent::Focused(false)).is_empty() && message(&ready, TerminalEvent::Bell).is_empty());
    }

    /// A long paste reaches the PTY as several writes, each a whole number of characters, nothing lost.
    #[test]
    fn a_long_paste_goes_as_several_inputs_cut_between_characters() {
        let ready = AtomicBool::new(true);
        let pasted = "가".repeat(INPUT_CHUNK * 2 + 5);
        let messages = message(&ready, TerminalEvent::Input(pasted.as_bytes().to_vec()));
        assert_eq!(messages.len(), 3);
        let texts: Vec<&str> = messages.iter().map(|m| m["text"].as_str().unwrap()).collect();
        assert_eq!(texts.iter().map(|t| t.chars().count()).collect::<Vec<_>>(), [INPUT_CHUNK, INPUT_CHUNK, 5]);
        assert_eq!(texts.concat(), pasted);
    }

    /// The view opens at the slot and its events reach its pane in order, the first grid included even when it came before the view
    /// was registered (Ghostty reports a size while its surface is made).
    #[tokio::test]
    async fn a_views_events_reach_its_pane_in_order() {
        let (natives, surfaces, log) = natives_with_log();
        let factory: Arc<dyn SurfaceFactory> = surfaces.clone();
        let view = natives.open(&factory, "pane", RECT).await.expect("a view");
        assert_eq!(
            surfaces.events()[0],
            SurfaceEvent::Opened { pane: "pane".into(), kind: mighty_platform::SurfaceKind::Terminal, bounds: RECT }
        );
        assert!(Arc::ptr_eq(&natives.view("pane").unwrap(), &view));
        surfaces.emit_terminal("pane", TerminalEvent::Resized { columns: 100, rows: 30, width: 0, height: 0 });
        surfaces.emit_terminal("pane", TerminalEvent::Input(b"echo hi\r".to_vec()));
        surfaces.emit_terminal("pane", TerminalEvent::Resized { columns: 90, rows: 28, width: 0, height: 0 });
        until(|| lock(&log).len() == 3).await;
        let types: Vec<(String, String)> =
            lock(&log).iter().map(|(pane, m)| (pane.clone(), m["type"].as_str().unwrap().to_string())).collect();
        assert_eq!(types, [("pane".into(), "ready".into()), ("pane".into(), "input".into()), ("pane".into(), "resize".into())]);
    }

    /// No view before the terminals start, and none when the platform gives none.
    #[tokio::test]
    async fn no_view_opens_without_the_terminals_or_a_platform_view() {
        let factory: Arc<dyn SurfaceFactory> = Arc::new(FakeSurfaces::new(true));
        assert!(NativeTerminals::default().open(&factory, "pane", RECT).await.is_none(), "the terminals have not started");
        let (natives, _, _) = natives_with_log();
        let web: Arc<dyn SurfaceFactory> = Arc::new(FakeSurfaces::new(false));
        assert!(natives.open(&web, "pane", RECT).await.is_none() && natives.view("pane").is_none(), "the page draws terminals here");
    }

    /// What the pane would push to the page's xterm goes to its view: the output fed as the bytes it was, a restart's reset, the theme
    /// and the font; the footer still goes to the page, and a pane without a view keeps the page's protocol.
    #[tokio::test]
    async fn the_panes_pushes_feed_its_view_and_the_footer_stays_the_pages() {
        let (natives, surfaces, _) = natives_with_log();
        let factory: Arc<dyn SurfaceFactory> = surfaces.clone();
        assert!(!natives.post("pane", &TerminalMessage::Reset), "no view: the page's");
        natives.open(&factory, "pane", RECT).await.unwrap();
        surfaces.clear();
        let data = base64::engine::general_purpose::STANDARD.encode("한글 ok\r\n".as_bytes());
        assert!(natives.post("pane", &TerminalMessage::Output { data }));
        assert!(natives.post("pane", &TerminalMessage::Reset));
        assert!(natives.post("pane", &TerminalMessage::Theme { light: true }));
        assert!(natives.post("pane", &TerminalMessage::Font { size: 13.5, family: Some("JetBrains Mono".into()) }));
        assert!(natives.post("pane", &TerminalMessage::Font { size: 12.0, family: None }));
        let footer = mighty_terminal::TerminalFooter {
            available: true,
            shell: true,
            notice: None,
            restart: false,
            restart_enabled: false,
            engine: "Ghostty".into(),
            title: String::new(),
            directory: String::new(),
            grid: String::new(),
        };
        assert!(!natives.post("pane", &TerminalMessage::Footer { footer }), "the page draws the footer");
        let pane = || "pane".to_string();
        assert_eq!(
            surfaces.events(),
            [
                SurfaceEvent::Fed { pane: pane(), output: "한글 ok\r\n".as_bytes().to_vec() },
                SurfaceEvent::Fed { pane: pane(), output: RESET.to_vec() },
                SurfaceEvent::Theme { pane: pane(), light: true },
                SurfaceEvent::Font { pane: pane(), family: "JetBrains Mono".into(), tenths: 135 },
                SurfaceEvent::Font { pane: pane(), family: String::new(), tenths: 120 },
            ]
        );
        natives.forget("pane");
        assert!(!natives.post("pane", &TerminalMessage::Reset), "a forgotten pane's messages are the page's again");
    }

    /// A shell pane over a loopback PTY (each write comes back as output) drawn by a native view: the pane, its view's registry
    /// (connected as terminal.rs connects it) and the PTY host.
    struct NativePane {
        natives: Arc<NativeTerminals>,
        surfaces: Arc<FakeSurfaces>,
        pane: Arc<mighty_terminal::TerminalPane>,
        ptys: Arc<mighty_platform::fake::FakePtyHost>,
    }

    /// terminal.rs' `HostEvents` as far as the view goes: what the pane posts reaches its view.
    struct ToView(Arc<NativeTerminals>);

    impl mighty_terminal::PaneEvents for ToView {
        fn post(&self, pane: &str, message: &TerminalMessage) {
            self.0.post(pane, message);
        }
        fn status(&self, _pane: &str, _status: &'static str) {}
        fn activity(&self, _pane: &str, _busy: bool) {}
        fn error(&self, _text: String) {}
        fn pressed(&self, _pane: &str) {}
        fn light(&self) -> bool {
            false
        }
        fn font(&self) -> mighty_terminal::TerminalFont {
            mighty_terminal::TerminalFont::default()
        }
        fn agent_terminal(&self, _owner: &str) -> Option<Arc<mighty_terminal::AgentTerminal>> {
            None
        }
    }

    impl NativePane {
        /// The pane with its view open and its shell started at the view's first grid.
        async fn start() -> NativePane {
            use mighty_platform::fake::{FakePtyHost, FakeShellEnvironment, MemoryClipboard};
            let natives = Arc::new(NativeTerminals::default());
            let surfaces = Arc::new(FakeSurfaces::new(true));
            let ptys = Arc::new(FakePtyHost::default());
            let ports = mighty_terminal::Ports {
                ptys: ptys.clone(),
                environment: Arc::new(FakeShellEnvironment::new(BTreeMap::new())),
                clipboard: Arc::new(MemoryClipboard::with_text("")),
                locale: mighty_core::locale::Locale::new("en"),
                engine: "Ghostty".into(),
            };
            let options = mighty_terminal::PaneOptions {
                id: "pane".into(),
                kind: mighty_terminal::PaneKind::Shell,
                folder: Some(std::env::temp_dir()),
                enabled: true,
                no_profile: true,
            };
            let pane = mighty_terminal::TerminalPane::new(options, ports, Arc::new(ToView(natives.clone())));
            let target = pane.clone();
            natives.connect(Arc::new(move |_pane, message| {
                let target = target.clone();
                Box::pin(async move { target.handle_message(&message).await })
            }));
            let factory: Arc<dyn SurfaceFactory> = surfaces.clone();
            natives.open(&factory, "pane", RECT).await.expect("a view");
            surfaces.emit_terminal("pane", TerminalEvent::Resized { columns: 90, rows: 25, width: 0, height: 0 });
            until(|| pane.is_running()).await;
            assert!(pane.is_running(), "the shell started at the view's first grid");
            assert_eq!(ptys.opened()[0].size, mighty_platform::PtySize { columns: 90, rows: 25 });
            NativePane { natives, surfaces, pane, ptys }
        }

        /// What the view was fed, the pane's output flushed first (its 32 ms clock).
        async fn drawn(&self, wanted: &str) -> String {
            let fed = || {
                let bytes: Vec<u8> = self
                    .surfaces
                    .events()
                    .into_iter()
                    .filter_map(|event| match event {
                        SurfaceEvent::Fed { output, .. } => Some(output),
                        _ => None,
                    })
                    .flatten()
                    .collect();
                String::from_utf8_lossy(&bytes).into_owned()
            };
            for _ in 0..200 {
                self.pane.tick();
                if fed().contains(wanted) {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }
            fed()
        }
    }

    /// Swift `TerminalInputPolicyTests.swift:15` "installIsNeverRun", on the path the app takes (styles.rs' `run_terminal_command`:
    /// `TerminalInput::apply` over `TerminalPane::write`) with the pane drawn natively: what came from a manifest reaches the shell and
    /// is drawn, with no Enter after it; only a command the app wrote itself is followed by one.
    #[tokio::test]
    async fn install_is_never_run() {
        let native = NativePane::start().await;
        let install = bundled_install_commands().into_iter().next().expect("a bundled install command");
        let typed = |text: &str| native.pane.write(text).unwrap_or(false);
        assert_eq!(TerminalInput::apply(&install, false, typed), TerminalInput::Pasted);
        let drawn = native.drawn(&install).await;
        assert!(drawn.contains(&install) && !drawn.contains('\r'), "put in, left for the user to run: {drawn:?}");
        assert_eq!(TerminalInput::apply("claude login", true, typed), TerminalInput::PastedAndRan);
        assert!(native.drawn("claude login\r").await.contains("claude login\r"), "the app's own command runs");
        assert!(native.natives.view("pane").is_some(), "the view stays the pane's");
    }

    /// Swift `TerminalInputPolicyTests.swift:27` "aLineBreakStopsThePasteAltogether", on the same path: a line break of any kind means
    /// nothing reaches the shell or the view at all, and text the terminal did not take (the shell has ended) is never followed by
    /// Enter.
    #[tokio::test]
    async fn a_line_break_stops_the_paste_altogether() {
        let native = NativePane::start().await;
        for separator in ["\n", "\r", "\u{2028}", "\u{2029}"] {
            let text = format!("rm -rf /{separator}echo done");
            let outcome = TerminalInput::apply(&text, true, |t| native.pane.write(t).unwrap_or(false));
            assert_eq!(outcome, TerminalInput::Refused, "{separator:?}");
        }
        assert!(!native.drawn("rm -rf").await.contains("rm -rf"), "nothing reached the shell");
        native.ptys.exit(0, 0);
        until(|| !native.pane.is_running()).await;
        let mut writes = 0;
        let outcome = TerminalInput::apply("ls", true, |t| {
            writes += 1;
            native.pane.write(t).unwrap_or(false)
        });
        assert!(outcome == TerminalInput::Failed && writes == 1, "an ended shell takes nothing, and no Enter follows");
    }

    /// Typing that is not UTF-8 (an X10 mouse report) reaches the PTY as the bytes it was, not through a lossy decoding.
    #[tokio::test]
    async fn bytes_that_are_not_utf8_reach_the_pty_as_they_were() {
        let ready = AtomicBool::new(true);
        let report = vec![0x1b, b'[', b'M', b' ', 0xff, 0xa1];
        let messages = message(&ready, TerminalEvent::Input(report.clone()));
        let data = base64::engine::general_purpose::STANDARD.encode(&report);
        assert_eq!(messages, [json!({"type": "inputBytes", "data": data})]);
        let native = NativePane::start().await;
        native.surfaces.emit_terminal("pane", TerminalEvent::Input(report));
        let drawn = native.drawn("\u{1b}[M ").await;
        assert!(drawn.contains("\u{1b}[M "), "the report came back through the loopback PTY: {drawn:?}");
        assert_eq!(native.pane.diagnostics()["inputs"], 1);
    }

    /// A view the platform could not open is not asked for again until its pane is forgotten.
    #[tokio::test]
    async fn a_view_that_failed_is_not_tried_again() {
        let (natives, surfaces, _) = natives_with_log();
        let factory: Arc<dyn SurfaceFactory> = surfaces.clone();
        surfaces.fail_next_open();
        assert!(natives.open(&factory, "pane", RECT).await.is_none());
        assert!(natives.open(&factory, "pane", RECT).await.is_none(), "remembered");
        assert!(surfaces.events().is_empty(), "the platform was asked once");
        natives.forget("pane");
        assert!(natives.open(&factory, "pane", RECT).await.is_some(), "a new pane of that id tries again");
    }

    /// Every bundled style's install command (mighty-styles' bundled manifests).
    fn bundled_install_commands() -> Vec<String> {
        mighty_styles::bundled::MANIFESTS
            .iter()
            .filter_map(|(_, bytes)| serde_json::from_slice::<Value>(bytes).ok())
            .filter_map(|manifest| manifest["install"]["command"].as_str().map(str::to_string))
            .collect()
    }

    /// Swift `TerminalInputPolicyTests.swift:38` "theBundledInstallCommandsAreSafeToPaste": every bundled style that offers an install
    /// command (at least one does) offers one line with no character the style text bans.
    #[test]
    fn the_bundled_install_commands_are_safe_to_paste() {
        let commands = bundled_install_commands();
        assert!(!commands.is_empty());
        for command in commands {
            assert!(!command.is_empty() && !TerminalInput::has_line_break(&command), "{command}");
            assert!(!command.chars().any(|c| mighty_styles::manifest::banned(c as u32)), "{command}");
        }
    }
}
