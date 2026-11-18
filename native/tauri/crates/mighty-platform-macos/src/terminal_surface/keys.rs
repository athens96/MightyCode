//! The view's keyboard rules with no AppKit in them, so they are tested as they are (the vendored `GhosttyTerminal`'s AppKit input,
//! `TerminalKeyEventHandler@AppKit.swift`, `TerminalInputText.swift`, `TerminalMarkedTextState.swift`, `TerminalKeyPress.swift`, and
//! Mighty's `ClipboardShortcut.swift`, `TranscriptCopyClaim.swift`): AppKit's modifier bits as Ghostty's, the text a key may carry,
//! the IME's marked text, ⌘C/⌘V by key position under a non-Latin input source, and text typed for the host as Ghostty's text and key
//! events.

/// `NSEvent.ModifierFlags` bits (`NSEventModifierFlags`).
pub const CAPS_LOCK: u64 = 1 << 16;
pub const SHIFT: u64 = 1 << 17;
pub const CONTROL: u64 = 1 << 18;
pub const OPTION: u64 = 1 << 19;
pub const COMMAND: u64 = 1 << 20;
pub const NUMERIC_PAD: u64 = 1 << 21;
pub const FUNCTION: u64 = 1 << 23;
/// The device-dependent bits of the right-hand modifiers (`NX_DEVICERSHIFTKEYMASK` …).
pub const RIGHT_SHIFT: u64 = 0x04;
pub const RIGHT_CONTROL: u64 = 0x2000;
pub const RIGHT_OPTION: u64 = 0x40;
pub const RIGHT_COMMAND: u64 = 0x10;

/// `ghostty_input_mods_e`.
pub const MODS_SHIFT: i32 = 1;
pub const MODS_CTRL: i32 = 1 << 1;
pub const MODS_ALT: i32 = 1 << 2;
pub const MODS_SUPER: i32 = 1 << 3;
pub const MODS_CAPS: i32 = 1 << 4;
pub const MODS_NUM: i32 = 1 << 5;

/// macOS virtual key codes the host presses itself.
pub const KEY_C: u16 = 8;
pub const KEY_V: u16 = 9;
pub const KEY_D: u16 = 2;
pub const KEY_RETURN: u16 = 36;
pub const KEY_TAB: u16 = 48;
pub const KEY_DELETE: u16 = 51;
pub const KEY_ESCAPE: u16 = 53;

/// `TerminalInputModifiers(from:).ghosttyMods`: AppKit's modifier flags as Ghostty's.
pub fn ghostty_mods(flags: u64) -> i32 {
    [(SHIFT, MODS_SHIFT), (CONTROL, MODS_CTRL), (OPTION, MODS_ALT), (COMMAND, MODS_SUPER), (CAPS_LOCK, MODS_CAPS), (NUMERIC_PAD, MODS_NUM)]
        .into_iter()
        .filter(|(flag, _)| flags & flag != 0)
        .fold(0, |mods, (_, mod_bit)| mods | mod_bit)
}

/// Ghostty's modifiers back as AppKit's (shift, control, option, command only: `eventModifierFlags(from:)`).
pub fn appkit_flags(mods: i32) -> u64 {
    [(MODS_SHIFT, SHIFT), (MODS_CTRL, CONTROL), (MODS_ALT, OPTION), (MODS_SUPER, COMMAND)]
        .into_iter()
        .filter(|(mod_bit, _)| mods & mod_bit != 0)
        .fold(0, |flags, (_, flag)| flags | flag)
}

/// The modifiers the key binding system treats as spent by the text (`buildKeyInput`'s consumed mods): all but control and command.
pub fn consumed_mods(flags: u64) -> i32 {
    ghostty_mods(flags & !(CONTROL | COMMAND))
}

/// `TerminalInputText.filteredFunctionKeyText`: a function key's private-use character (U+F700…U+F8FF) is no text.
pub fn filtered_text(text: Option<&str>) -> Option<&str> {
    let text = text?;
    let mut chars = text.chars();
    match (chars.next(), chars.next()) {
        (Some(c), None) if ('\u{F700}'..='\u{F8FF}').contains(&c) => None,
        _ => Some(text),
    }
}

/// Whether the one character of `text` is an ASCII control character (Ctrl turned a letter into one: the view then asks AppKit for
/// the key's text without Ctrl, so Ghostty maps the physical key).
pub fn is_control_character(text: &str) -> bool {
    let mut chars = text.chars();
    matches!((chars.next(), chars.next()), (Some(c), None) if (c as u32) < 0x20)
}

/// Which side a modifier key belongs to, and its Ghostty bit (`handleFlagsChanged`): None for any other key.
pub fn modifier_key(key_code: u16) -> Option<(i32, Option<u64>)> {
    Some(match key_code {
        0x39 => (MODS_CAPS, None),
        0x38 => (MODS_SHIFT, None),
        0x3C => (MODS_SHIFT, Some(RIGHT_SHIFT)),
        0x3B => (MODS_CTRL, None),
        0x3E => (MODS_CTRL, Some(RIGHT_CONTROL)),
        0x3A => (MODS_ALT, None),
        0x3D => (MODS_ALT, Some(RIGHT_OPTION)),
        0x37 => (MODS_SUPER, None),
        0x36 => (MODS_SUPER, Some(RIGHT_COMMAND)),
        _ => return None,
    })
}

/// Whether a modifier key's change is a press (`handleFlagsChanged`): its bit is now set and, for a right-hand key, its own device
/// bit too.
pub fn modifier_pressed(key_code: u16, flags: u64) -> Option<bool> {
    let (bit, side) = modifier_key(key_code)?;
    Some(ghostty_mods(flags) & bit != 0 && side.is_none_or(|side| flags & side != 0))
}

/// `TerminalScrollModifiers(precision:momentum:)` from an `NSEvent`'s precise deltas and momentum phase (`NSEventPhase` bits:
/// began 1, stationary 2, changed 4).
pub fn scroll_mods(precise: bool, momentum_phase: u64) -> i32 {
    let momentum = if momentum_phase & 1 != 0 {
        1
    } else if momentum_phase & 2 != 0 {
        2
    } else if momentum_phase & 4 != 0 {
        3
    } else {
        0
    };
    i32::from(precise) | (momentum << 1)
}

/// What [`clipboard_shortcut`] recognised.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClipboardKey {
    Copy,
    Paste,
}

/// Swift `ClipboardShortcut.match`: Command and nothing else (Caps Lock, Fn and the numeric-pad bit cleared first), on C or V by its
/// character, or — when the input source gives no Latin character at all (Korean's "ㅊ") — by the physical key (8, 9). A Latin layout
/// that puts another character there keeps its own shortcut.
pub fn clipboard_shortcut(modifiers: u64, characters: Option<&str>, key_code: u16) -> Option<ClipboardKey> {
    let meaningful = modifiers & 0xFFFF_0000 & !(CAPS_LOCK | NUMERIC_PAD | FUNCTION);
    if meaningful != COMMAND {
        return None;
    }
    let characters = characters.unwrap_or("").to_lowercase();
    match characters.as_str() {
        "c" => return Some(ClipboardKey::Copy),
        "v" => return Some(ClipboardKey::Paste),
        _ => {}
    }
    if characters.chars().any(|c| matches!(c as u32, 0x20..=0x024F | 0x1E00..=0x1EFF)) {
        return None;
    }
    match key_code {
        KEY_C => Some(ClipboardKey::Copy),
        KEY_V => Some(ClipboardKey::Paste),
        _ => None,
    }
}

/// Swift `TranscriptCopyClaim.isCopyKeyEquivalent`: Command alone on the C key, by character or by position.
pub fn is_copy_key_equivalent(modifiers: u64, characters: Option<&str>, key_code: u16) -> bool {
    clipboard_shortcut(modifiers, characters, key_code) == Some(ClipboardKey::Copy)
}

/// One step of text the host types into the terminal ([`typed`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Typed {
    /// Ghostty's text input (`ghostty_surface_text`).
    Text(String),
    /// A key press and release (`ghostty_surface_key`), as `TerminalKeyPress` builds it: the key's code, its modifiers, the text the
    /// press types and its unshifted codepoint.
    Key { key_code: u16, mods: i32, text: &'static str, unshifted: u32 },
}

/// The key a control character is typed with: CR or LF Return, Tab, DEL or BS Delete, ESC Escape, Ctrl+C, Ctrl+D.
fn control_key(c: char) -> Option<Typed> {
    let key = |key_code, mods, text: &'static str, unshifted| Typed::Key { key_code, mods, text, unshifted };
    Some(match c {
        '\r' | '\n' => key(KEY_RETURN, 0, "\r", 13),
        '\t' => key(KEY_TAB, 0, "\t", 9),
        '\u{7f}' | '\u{8}' => key(KEY_DELETE, 0, "\u{7f}", 127),
        '\u{1b}' => key(KEY_ESCAPE, 0, "\u{1b}", 27),
        '\u{3}' => key(KEY_C, MODS_CTRL, "c", 'c' as u32),
        '\u{4}' => key(KEY_D, MODS_CTRL, "d", 'd' as u32),
        _ => return None,
    })
}

/// `text` as the host types it (`TerminalSurface::send_text`): runs of text as Ghostty's text input, each control character among
/// them as the key that types it, so a CR is the Return key (never a pasted CR, which a shell taking bracketed pastes would keep in
/// its edit line) and `\u{3}` is Ctrl+C. A CR LF pair is one Return.
pub fn typed(text: &str) -> Vec<Typed> {
    let mut steps = Vec::new();
    let mut run = String::new();
    let mut previous = '\0';
    for c in text.chars() {
        match control_key(c) {
            Some(_) if c == '\n' && previous == '\r' => {}
            Some(key) => {
                if !run.is_empty() {
                    steps.push(Typed::Text(std::mem::take(&mut run)));
                }
                steps.push(key);
            }
            None => run.push(c),
        }
        previous = c;
    }
    if !run.is_empty() {
        steps.push(Typed::Text(run));
    }
    steps
}

/// The IME's marked (composing) text (`TerminalMarkedTextState`): its text and the selection in it, in UTF-16 units as AppKit counts.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MarkedText {
    text: Option<String>,
    selected: (usize, usize),
}

/// `NSNotFound` (as `NSRange.location`).
pub const NOT_FOUND: usize = isize::MAX as usize;

impl MarkedText {
    pub fn has_marked_text(&self) -> bool {
        self.text.as_deref().is_some_and(|text| !text.is_empty())
    }

    pub fn text(&self) -> Option<&str> {
        self.text.as_deref().filter(|text| !text.is_empty())
    }

    fn length(&self) -> usize {
        self.text.as_deref().map_or(0, |text| text.encode_utf16().count())
    }

    /// `setMarkedText(_:selectedRange:)`: an empty text is none; the selection is held inside the text.
    pub fn set(&mut self, text: &str, selected: (usize, usize)) {
        self.text = (!text.is_empty()).then(|| text.to_string());
        let length = self.length();
        let location = selected.0.min(length);
        let end = selected.0.saturating_add(selected.1).clamp(location, length);
        self.selected = (location, end - location);
    }

    pub fn clear(&mut self) {
        self.text = None;
        self.selected = (0, 0);
    }

    /// `markedRange`: the whole text, or `(NSNotFound, 0)` without one.
    pub fn marked_range(&self) -> (usize, usize) {
        if self.has_marked_text() { (0, self.length()) } else { (NOT_FOUND, 0) }
    }

    /// `selectedRange`: the selection in the text, or `(NSNotFound, 0)` without one.
    pub fn selected_range(&self) -> (usize, usize) {
        if self.has_marked_text() { self.selected } else { (NOT_FOUND, 0) }
    }

    /// `attributedSubstring(forProposedRange:)`: the part of the text in `range`, held inside it, and the range it really is.
    pub fn substring(&self, range: (usize, usize)) -> Option<(String, (usize, usize))> {
        let text = self.text()?;
        let units: Vec<u16> = text.encode_utf16().collect();
        let location = range.0.min(units.len());
        let end = range.0.saturating_add(range.1).clamp(location, units.len());
        Some((String::from_utf16_lossy(&units[location..end]), (location, end - location)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NUMERIC_PAD_BIT: u64 = NUMERIC_PAD;

    /// Swift `TranscriptCopyTests.swift:15` "onlyCommandAloneOnTheCKeyCountsAsCopy": Command on C, by character or (for Korean "ㅊ" or
    /// no character) by position; a layout's own "c" elsewhere still copies; other modifiers, no modifier or V are no copy.
    #[test]
    fn only_command_alone_on_the_c_key_counts_as_copy() {
        assert!(is_copy_key_equivalent(COMMAND, Some("c"), 8));
        assert!(is_copy_key_equivalent(COMMAND, Some("ㅊ"), 8));
        assert!(is_copy_key_equivalent(COMMAND, None, 8));
        assert!(is_copy_key_equivalent(COMMAND, Some("c"), 55));
        assert!(!is_copy_key_equivalent(COMMAND | SHIFT, Some("C"), 8));
        assert!(!is_copy_key_equivalent(COMMAND | OPTION, Some("ç"), 8));
        assert!(!is_copy_key_equivalent(0, Some("c"), 8));
        assert!(!is_copy_key_equivalent(COMMAND, Some("v"), 9));
        assert_eq!(clipboard_shortcut(COMMAND, Some("ㅍ"), 9), Some(ClipboardKey::Paste), "Korean V pastes by position");
        assert_eq!(clipboard_shortcut(COMMAND, Some("j"), 8), None, "a Latin layout's own letter there keeps its shortcut");
    }

    /// Swift `TranscriptCopyTests.swift:33` "incidentalModifiersDoNotDefeatDetection": Caps Lock, Fn and the numeric-pad bit are
    /// cleared before the comparison, never allowed to defeat it; a real extra modifier still disqualifies.
    #[test]
    fn incidental_modifiers_do_not_defeat_detection() {
        assert!(is_copy_key_equivalent(COMMAND | CAPS_LOCK, Some("ㅊ"), 8));
        assert!(is_copy_key_equivalent(COMMAND | FUNCTION, Some("c"), 8));
        assert!(is_copy_key_equivalent(COMMAND | NUMERIC_PAD_BIT, Some("c"), 8));
        assert!(is_copy_key_equivalent(COMMAND | CAPS_LOCK | FUNCTION | NUMERIC_PAD_BIT, Some("ㅊ"), 8));
        assert!(!is_copy_key_equivalent(COMMAND | CAPS_LOCK | SHIFT, Some("c"), 8));
        assert!(!is_copy_key_equivalent(CAPS_LOCK | FUNCTION, Some("c"), 8));
    }

    /// The host's text reaches Ghostty as text with each control character pressed as its key: CR (and CR LF, once) is Return, ETX
    /// Ctrl+C, EOT Ctrl+D, Tab, Delete and Escape their keys; Korean text stays one text run.
    #[test]
    fn typed_text_presses_its_control_characters_as_keys() {
        let ret = Typed::Key { key_code: KEY_RETURN, mods: 0, text: "\r", unshifted: 13 };
        assert_eq!(typed("echo 한글\r"), [Typed::Text("echo 한글".into()), ret.clone()]);
        assert_eq!(typed("a\r\nb\n"), [Typed::Text("a".into()), ret.clone(), Typed::Text("b".into()), ret.clone()]);
        assert_eq!(typed("\u{3}"), [Typed::Key { key_code: KEY_C, mods: MODS_CTRL, text: "c", unshifted: 'c' as u32 }]);
        assert_eq!(typed("\u{4}")[0], Typed::Key { key_code: KEY_D, mods: MODS_CTRL, text: "d", unshifted: 'd' as u32 });
        assert_eq!(
            typed("\tx\u{7f}\u{1b}"),
            [
                Typed::Key { key_code: KEY_TAB, mods: 0, text: "\t", unshifted: 9 },
                Typed::Text("x".into()),
                Typed::Key { key_code: KEY_DELETE, mods: 0, text: "\u{7f}", unshifted: 127 },
                Typed::Key { key_code: KEY_ESCAPE, mods: 0, text: "\u{1b}", unshifted: 27 },
            ]
        );
        assert!(typed("").is_empty());
    }

    /// AppKit's flags as Ghostty's and back; the text keeps every modifier but Control and Command for the bindings; a function key's
    /// character is no text; a right-hand modifier presses only with its own device bit.
    #[test]
    fn modifiers_and_key_text_follow_ghosttys_appkit_view() {
        assert_eq!(
            ghostty_mods(SHIFT | CONTROL | OPTION | COMMAND | CAPS_LOCK),
            MODS_SHIFT | MODS_CTRL | MODS_ALT | MODS_SUPER | MODS_CAPS
        );
        assert_eq!(appkit_flags(MODS_SHIFT | MODS_SUPER | MODS_CAPS), SHIFT | COMMAND);
        assert_eq!(consumed_mods(SHIFT | CONTROL | OPTION | COMMAND), MODS_SHIFT | MODS_ALT);
        assert_eq!(filtered_text(Some("\u{F700}")), None, "the up arrow's private-use character");
        assert_eq!(filtered_text(Some("a")), Some("a"));
        assert!(is_control_character("\u{3}") && !is_control_character("c") && !is_control_character("ab"));
        assert_eq!(modifier_pressed(0x38, SHIFT), Some(true));
        assert_eq!(modifier_pressed(0x3C, SHIFT), Some(false), "the left shift's bit alone is no right shift press");
        assert_eq!(modifier_pressed(0x3C, SHIFT | RIGHT_SHIFT), Some(true));
        assert_eq!(modifier_pressed(0x37, 0), Some(false), "released");
        assert_eq!(modifier_pressed(0x00, COMMAND), None, "not a modifier key");
        assert_eq!(scroll_mods(true, 0), 1);
        assert_eq!(scroll_mods(false, 4), 3 << 1);
        assert_eq!(scroll_mods(true, 1), 1 | (1 << 1));
    }

    /// The IME's marked text (`TerminalMarkedTextState`): ranges in UTF-16 units, the selection held inside the text, nothing marked
    /// answering `NSNotFound`, a substring clamped to the text.
    #[test]
    fn marked_text_ranges_are_utf16_and_held_inside_the_text() {
        let mut marked = MarkedText::default();
        assert!(!marked.has_marked_text());
        assert_eq!((marked.marked_range(), marked.selected_range()), ((NOT_FOUND, 0), (NOT_FOUND, 0)));
        marked.set("한", (1, 0));
        assert_eq!((marked.marked_range(), marked.selected_range(), marked.text()), ((0, 1), (1, 0), Some("한")));
        marked.set("😀가", (5, 9));
        assert_eq!(marked.marked_range(), (0, 3), "an emoji is two units");
        assert_eq!(marked.selected_range(), (3, 0));
        assert_eq!(marked.substring((2, 10)), Some(("가".to_string(), (2, 1))));
        marked.set("", (0, 0));
        assert!(!marked.has_marked_text() && marked.substring((0, 1)).is_none());
        marked.set("ㅎ", (1, 0));
        marked.clear();
        assert_eq!(marked, MarkedText::default());
    }
}
