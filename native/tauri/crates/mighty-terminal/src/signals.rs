//! A terminal's "look at me" signals (MC-26): the desktop notifications a program asks for with OSC 9 (iTerm2's `ESC ] 9 ; body`, not
//! ConEmu's numbered forms such as the `9;4;…` progress), OSC 777 (`ESC ] 777 ; notify ; title ; body`) and OSC 99 (kitty's, only its
//! title and body), and the bell. Pure: [`OscScanner`] reads a PTY's output as it comes (a sequence split between two reads is still
//! found), [`parse_osc`] reads one finished sequence, [`sanitize`] makes its words safe to show.
//!
//! Where the page draws the terminal (Windows: xterm.js) the page finds them itself (src/panes/terminal/signals.ts) and sends
//! `terminalMessage {type:'notify'|'bell'}`; the Mac's Ghostty gives the bell and OSC 9/777 as its own actions, and the host's scanner
//! finds the OSC 99 Ghostty leaves alone.

use std::collections::HashMap;

use base64::Engine;

/// The most characters of a signal's words kept after [`sanitize`].
pub const TEXT_LIMIT: usize = 200;
/// The longest OSC string read, in bytes: a longer one is dropped whole.
const OSC_LIMIT: usize = 4096;
/// The most kitty notifications waiting for their last chunk (`d=0`) at once.
const KITTY_PENDING: usize = 8;

/// What a terminal asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Signal {
    /// A desktop notification: its title (empty when it gave none) and its body, both as the program wrote them.
    Notify { title: String, body: String },
    /// The bell (BEL alone, never the BEL that ends an OSC string).
    Bell,
}

impl Signal {
    /// The words to show for a notification, made safe: its body, else its title.
    pub fn text(&self) -> String {
        match self {
            Signal::Notify { title, body } => {
                let body = sanitize(body);
                if body.is_empty() { sanitize(title) } else { body }
            }
            Signal::Bell => String::new(),
        }
    }
}

/// `text` safe for one line: control characters become spaces, runs of white space one space, trimmed, at most [`TEXT_LIMIT`]
/// characters.
pub fn sanitize(text: &str) -> String {
    let mut out = String::new();
    let mut space = false;
    for c in text.chars() {
        // Bidirectional overrides and isolates could make the words read differently from what they are.
        let hidden = matches!(c, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}' | '\u{200b}' | '\u{feff}');
        if hidden {
            continue;
        }
        if c.is_control() || c.is_whitespace() {
            space = !out.is_empty();
            continue;
        }
        if space {
            out.push(' ');
            space = false;
        }
        out.push(c);
    }
    match out.char_indices().nth(TEXT_LIMIT) {
        Some((at, _)) => out[..at].trim_end().to_string(),
        None => out,
    }
}

/// One kitty (OSC 99) chunk: which notification, whether it is the last chunk, and what it carries.
#[derive(Debug, Clone, PartialEq, Eq)]
struct KittyChunk {
    id: String,
    done: bool,
    /// `p=title` (the default) or `p=body`; any other payload (`close`, `icon`, `buttons`, `?` …) is None and ignored.
    part: Option<KittyPart>,
    text: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum KittyPart {
    Title,
    Body,
}

/// What one finished OSC string is.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Osc {
    Signal(Signal),
    Kitty(KittyChunk),
}

fn read_osc(payload: &str) -> Option<Osc> {
    let (code, rest) = payload.split_once(';').unwrap_or((payload, ""));
    match code {
        "9" => {
            // ConEmu's numbered forms (`9;4;st;pr` progress, `9;1;ms` sleep, …) are not notifications.
            let number = rest.split(';').next().unwrap_or("");
            let numbered = (1..=2).contains(&number.len()) && number.bytes().all(|b| b.is_ascii_digit());
            (!numbered).then(|| Osc::Signal(Signal::Notify { title: String::new(), body: rest.to_string() }))
        }
        "777" => {
            let mut parts = rest.splitn(3, ';');
            if parts.next() != Some("notify") {
                return None;
            }
            let title = parts.next().unwrap_or("").to_string();
            let body = parts.next().unwrap_or("").to_string();
            Some(Osc::Signal(Signal::Notify { title, body }))
        }
        "99" => {
            let (metadata, text) = rest.split_once(';').unwrap_or((rest, ""));
            let (mut id, mut done, mut part, mut base64) = (String::new(), true, Some(KittyPart::Title), false);
            for pair in metadata.split(':').filter(|p| !p.is_empty()) {
                let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
                match key {
                    "i" => {
                        id = value.chars().filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '+' | '.')).take(64).collect()
                    }
                    "d" => done = value != "0",
                    "e" => base64 = value == "1",
                    "p" => {
                        part = match value {
                            "title" => Some(KittyPart::Title),
                            "body" => Some(KittyPart::Body),
                            _ => None,
                        }
                    }
                    _ => {}
                }
            }
            let text = if base64 {
                let bytes = base64::engine::general_purpose::STANDARD.decode(text.trim()).ok()?;
                String::from_utf8_lossy(&bytes).into_owned()
            } else {
                text.to_string()
            };
            Some(Osc::Kitty(KittyChunk { id, done, part, text }))
        }
        _ => None,
    }
}

/// One finished OSC string's payload (what lies between `ESC ]` and its BEL or ST) as a notification: OSC 9 and 777 at once, a
/// kitty OSC 99 only when it is whole in one string (`d` absent or 1, its title or body). [`OscScanner`] also joins kitty's chunks.
pub fn parse_osc(payload: &str) -> Option<Signal> {
    match read_osc(payload)? {
        Osc::Signal(signal) => Some(signal),
        Osc::Kitty(chunk) => {
            let mut kitty = Kitty::default();
            kitty.take(chunk)
        }
    }
}

/// kitty's notifications still waiting for their last chunk, by id.
#[derive(Debug, Default, Clone)]
struct Kitty {
    pending: HashMap<String, (String, String)>,
}

impl Kitty {
    fn take(&mut self, chunk: KittyChunk) -> Option<Signal> {
        if !self.pending.contains_key(&chunk.id) && self.pending.len() >= KITTY_PENDING {
            self.pending.clear();
        }
        let (title, body) = self.pending.entry(chunk.id.clone()).or_default();
        let into = match chunk.part {
            Some(KittyPart::Title) => Some(title),
            Some(KittyPart::Body) => Some(body),
            None => None,
        };
        if let Some(into) = into
            && into.len() < OSC_LIMIT
        {
            into.push_str(&chunk.text);
        }
        if !chunk.done {
            return None;
        }
        let (title, body) = self.pending.remove(&chunk.id).unwrap_or_default();
        // A chunk of another kind alone (a close, an icon, a query) is no notification.
        (chunk.part.is_some() || !title.is_empty() || !body.is_empty()).then_some(Signal::Notify { title, body })
    }
}

/// Where the scanner stands between two reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum State {
    #[default]
    Ground,
    /// After ESC.
    Escape,
    /// Inside `ESC ]`, up to BEL or ST.
    Osc,
    /// An ESC inside the OSC string: `\` ends it.
    OscEscape,
    /// Inside a DCS, PM, APC or SOS string (`ESC P`, `ESC ^`, `ESC _`, `ESC X`), skipped to BEL or ST: a BEL there is no bell.
    Skip,
    SkipEscape,
}

/// Which signals a scanner reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Kinds {
    pub bell: bool,
    /// OSC 9 and OSC 777.
    pub notify: bool,
    /// OSC 99.
    pub kitty: bool,
}

impl Kinds {
    /// Everything (a terminal whose engine reports none of them).
    pub const ALL: Kinds = Kinds { bell: true, notify: true, kitty: true };
    /// OSC 99 alone: the Mac's Ghostty reports the bell and OSC 9/777 itself.
    pub const KITTY: Kinds = Kinds { bell: false, notify: false, kitty: true };
}

/// Finds the signals in a terminal's output as it comes, a sequence split across reads included.
#[derive(Debug, Clone)]
pub struct OscScanner {
    kinds: Kinds,
    state: State,
    buffer: Vec<u8>,
    overflow: bool,
    kitty: Kitty,
}

impl OscScanner {
    pub fn new(kinds: Kinds) -> OscScanner {
        OscScanner { kinds, state: State::Ground, buffer: Vec::new(), overflow: false, kitty: Kitty::default() }
    }

    /// Forgets a sequence it was inside (a terminal reset).
    pub fn reset(&mut self) {
        self.state = State::Ground;
        self.buffer.clear();
        self.overflow = false;
    }

    /// The signals `bytes` completes, in order.
    pub fn feed(&mut self, bytes: &[u8]) -> Vec<Signal> {
        let mut found = Vec::new();
        for &byte in bytes {
            self.state = match (self.state, byte) {
                (State::Ground, 0x1b) => State::Escape,
                (State::Ground, 0x07) => {
                    if self.kinds.bell {
                        found.push(Signal::Bell);
                    }
                    State::Ground
                }
                (State::Ground, _) => State::Ground,
                (State::Escape, b']') => {
                    self.buffer.clear();
                    self.overflow = false;
                    State::Osc
                }
                (State::Escape, b'P' | b'^' | b'_' | b'X') => State::Skip,
                (State::Escape, 0x1b) => State::Escape,
                (State::Escape, _) => State::Ground,
                (State::Osc, 0x07) => {
                    self.finish(&mut found);
                    State::Ground
                }
                (State::Osc, 0x1b) => State::OscEscape,
                // CAN and SUB cancel the string.
                (State::Osc, 0x18 | 0x1a) => State::Ground,
                (State::Osc, _) => {
                    if self.buffer.len() < OSC_LIMIT {
                        self.buffer.push(byte);
                    } else {
                        self.overflow = true;
                    }
                    State::Osc
                }
                (State::OscEscape, b'\\') => {
                    self.finish(&mut found);
                    State::Ground
                }
                // Any other ESC cancels the string and starts a sequence of its own.
                (State::OscEscape, b']') => {
                    self.buffer.clear();
                    self.overflow = false;
                    State::Osc
                }
                (State::OscEscape, 0x1b) => State::Escape,
                (State::OscEscape, _) => State::Ground,
                (State::Skip, 0x07 | 0x18 | 0x1a) => State::Ground,
                (State::Skip, 0x1b) => State::SkipEscape,
                (State::Skip, _) => State::Skip,
                (State::SkipEscape, b'\\') => State::Ground,
                (State::SkipEscape, 0x1b) => State::SkipEscape,
                (State::SkipEscape, _) => State::Skip,
            };
        }
        found
    }

    fn finish(&mut self, found: &mut Vec<Signal>) {
        let payload = std::mem::take(&mut self.buffer);
        if std::mem::take(&mut self.overflow) {
            return;
        }
        let text = String::from_utf8_lossy(&payload);
        match read_osc(&text) {
            Some(Osc::Signal(signal)) if self.kinds.notify => found.push(signal),
            Some(Osc::Kitty(chunk)) if self.kinds.kitty => found.extend(self.kitty.take(chunk)),
            _ => {}
        }
    }
}

/// At most one signal a pane per second: the time of the last one let through, by pane.
#[derive(Debug, Default, Clone)]
pub struct SignalLimiter {
    last: HashMap<String, std::time::Instant>,
}

/// The shortest gap between two signals of one pane.
pub const SIGNAL_GAP: std::time::Duration = std::time::Duration::from_secs(1);

impl SignalLimiter {
    /// Whether `pane`'s signal at `now` goes through (and it is remembered); false within [`SIGNAL_GAP`] of the last.
    pub fn allow(&mut self, pane: &str, now: std::time::Instant) -> bool {
        if let Some(last) = self.last.get(pane)
            && now.saturating_duration_since(*last) < SIGNAL_GAP
        {
            return false;
        }
        self.last.insert(pane.to_string(), now);
        true
    }

    /// The pane is gone.
    pub fn forget(&mut self, pane: &str) {
        self.last.remove(pane);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn notify(title: &str, body: &str) -> Signal {
        Signal::Notify { title: title.into(), body: body.into() }
    }

    #[test]
    fn osc_9_777_and_99_are_notifications() {
        assert_eq!(parse_osc("9;빌드 끝"), Some(notify("", "빌드 끝")));
        assert_eq!(parse_osc("9;3 tests failed"), Some(notify("", "3 tests failed")), "words after a number are a body");
        assert_eq!(parse_osc("777;notify;Build;done; all green"), Some(notify("Build", "done; all green")));
        assert_eq!(parse_osc("777;other;x;y"), None);
        assert_eq!(parse_osc("99;;Hello"), Some(notify("Hello", "")), "kitty's payload is the title by default");
        assert_eq!(parse_osc("99;i=1:p=body;World"), Some(notify("", "World")));
        assert_eq!(parse_osc("99;e=1:p=body;7JWI64WV"), Some(notify("", "안녕")), "base64");
        assert_eq!(parse_osc("99;i=1:p=close;"), None, "a close is no notification");
        assert_eq!(parse_osc("0;title"), None);
        assert_eq!(parse_osc("7;file:///tmp"), None);
    }

    #[test]
    fn conemu_numbered_osc_9_is_ignored() {
        assert_eq!(parse_osc("9;4;1;50"), None, "progress");
        assert_eq!(parse_osc("9;4;0"), None);
        assert_eq!(parse_osc("9;1;500"), None, "sleep");
        assert_eq!(parse_osc("9;12"), None);
    }

    #[test]
    fn the_scanner_finds_signals_split_across_reads() {
        let mut scanner = OscScanner::new(Kinds::ALL);
        assert!(scanner.feed(b"out\x1b]9;bu").is_empty());
        assert!(scanner.feed(b"ild done\x1b").is_empty());
        assert_eq!(scanner.feed(b"\\more"), [notify("", "build done")], "ended by ST across the reads");
        assert_eq!(scanner.feed(b"\x1b]777;notify;T;B\x07\x07"), [notify("T", "B"), Signal::Bell], "the OSC's BEL is no bell");
        assert_eq!(scanner.feed(b"\x1b]9;4;1;50\x07"), [], "progress");
        assert_eq!(scanner.feed(b"\x1b]0;title\x07\x1b[31mred\x1b[0m"), [], "a title and colours");
        assert_eq!(scanner.feed(b"\x1bPq#0\x07"), [], "a DCS string's BEL is no bell");
        assert_eq!(scanner.feed("\x1b]9;한글 알림\x07".as_bytes()), [notify("", "한글 알림")]);
        let split = "\x1b]9;끝\x07".as_bytes();
        assert!(scanner.feed(&split[..6]).is_empty(), "inside a character");
        assert_eq!(scanner.feed(&split[6..]), [notify("", "끝")]);
    }

    #[test]
    fn kitty_chunks_are_joined_and_a_cancelled_string_says_nothing() {
        let mut scanner = OscScanner::new(Kinds::ALL);
        assert!(scanner.feed(b"\x1b]99;i=7:d=0;Deploy\x1b\\").is_empty());
        assert_eq!(scanner.feed(b"\x1b]99;i=7:p=body;finished\x1b\\"), [notify("Deploy", "finished")]);
        assert!(scanner.feed(b"\x1b]9;lost\x18\x07").ends_with(&[Signal::Bell]), "CAN cancels; the next BEL is a bell");
        let long = format!("\x1b]9;{}\x07", "x".repeat(OSC_LIMIT + 10));
        assert!(scanner.feed(long.as_bytes()).is_empty(), "too long: dropped whole");
        assert_eq!(scanner.feed(b"\x1b]9;after\x07"), [notify("", "after")]);
    }

    #[test]
    fn a_scanner_reports_only_its_kinds() {
        let mut scanner = OscScanner::new(Kinds::KITTY);
        assert!(scanner.feed(b"\x07\x1b]9;x\x07\x1b]777;notify;a;b\x07").is_empty());
        assert_eq!(scanner.feed(b"\x1b]99;;Hi\x07"), [notify("Hi", "")]);
    }

    #[test]
    fn words_are_sanitized_and_capped() {
        assert_eq!(sanitize("  a\tb\r\n\x1b[31mc\u{7}  "), "a b [31mc");
        assert_eq!(sanitize("\u{202e}evil"), "evil");
        let long = "가".repeat(300);
        assert_eq!(sanitize(&long).chars().count(), TEXT_LIMIT);
        assert_eq!(notify("Title", "  ").text(), "Title", "an empty body shows the title");
        assert_eq!(notify("T", "body").text(), "body");
    }

    #[test]
    fn one_signal_a_pane_per_second() {
        let mut limiter = SignalLimiter::default();
        let start = std::time::Instant::now();
        assert!(limiter.allow("a", start));
        assert!(!limiter.allow("a", start + std::time::Duration::from_millis(500)));
        assert!(limiter.allow("b", start), "another pane");
        assert!(limiter.allow("a", start + SIGNAL_GAP));
        limiter.forget("a");
        assert!(limiter.allow("a", start + SIGNAL_GAP));
    }
}
