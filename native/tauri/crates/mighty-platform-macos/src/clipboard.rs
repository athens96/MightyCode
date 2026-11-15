//! M-A: the pasteboard (`NSPasteboard` text: read, write, clear), the host's copy and paste (the composer's, the files pane's, a
//! terminal's through the page); Ghostty's own go through M-D's view. Swift's `NSPasteboard.general` uses and
//! `ClipboardShortcut.swift`; contract `mighty_platform::clipboard`.
//!
//! A secret (a remote workspace's connection key) is written for this Mac only (`NSPasteboardContentsCurrentHostOnly`: no Universal
//! Clipboard) and marked concealed and transient (nspasteboard.org's `org.nspasteboard.ConcealedType` and `TransientType`, which
//! clipboard managers and history tools honour): the Mac's counterpart of Windows' excluded-from-history format.

use std::sync::Arc;

use mighty_platform::{Clipboard, PlatformError, Result};
use objc2::rc::Retained;
use objc2_app_kit::{NSPasteboard, NSPasteboardContentsOptions, NSPasteboardTypeString};
use objc2_foundation::NSString;

/// What clipboard managers skip (nspasteboard.org).
const CONCEALED: &str = "org.nspasteboard.ConcealedType";
const TRANSIENT: &str = "org.nspasteboard.TransientType";

/// The Mac's `Clipboard`: the general pasteboard.
pub fn clipboard() -> Arc<dyn Clipboard> {
    Arc::new(MacClipboard::general())
}

/// A pasteboard by name: the general one, or one of a test's own.
pub struct MacClipboard {
    name: Option<String>,
}

impl MacClipboard {
    pub fn general() -> MacClipboard {
        MacClipboard { name: None }
    }

    /// A pasteboard of this name (a test's, so the user's clipboard is never touched); [`MacClipboard::release`] removes it.
    pub fn named(name: &str) -> MacClipboard {
        MacClipboard { name: Some(name.to_string()) }
    }

    fn board(&self) -> Retained<NSPasteboard> {
        match &self.name {
            None => NSPasteboard::generalPasteboard(),
            Some(name) => NSPasteboard::pasteboardWithName(&NSString::from_str(name)),
        }
    }

    /// Gives a named pasteboard back to the system.
    pub fn release(&self) {
        if self.name.is_some() {
            let board = self.board();
            // SAFETY: `releaseGlobally` takes nothing and returns nothing (objc2-app-kit does not bind it).
            let () = unsafe { objc2::msg_send![&*board, releaseGlobally] };
        }
    }
}

fn string_type() -> &'static NSString {
    // SAFETY: an AppKit constant, valid for the process's life.
    unsafe { NSPasteboardTypeString }
}

impl Clipboard for MacClipboard {
    fn read_text(&self) -> Result<Option<String>> {
        Ok(self.board().stringForType(string_type()).map(|text| text.to_string()))
    }

    fn write_text(&self, text: &str) -> Result<()> {
        let board = self.board();
        board.clearContents();
        if !board.setString_forType(&NSString::from_str(text), string_type()) {
            return Err(PlatformError::Refused("the pasteboard did not take the text".into()));
        }
        Ok(())
    }

    fn write_secret(&self, text: &str) -> Result<()> {
        let board = self.board();
        board.prepareForNewContentsWithOptions(NSPasteboardContentsOptions::CurrentHostOnly);
        let written = board.setString_forType(&NSString::from_str(text), string_type())
            && board.setString_forType(&NSString::from_str(""), &NSString::from_str(CONCEALED))
            && board.setString_forType(&NSString::from_str(""), &NSString::from_str(TRANSIENT));
        if !written {
            board.clearContents();
            return Err(PlatformError::Refused("the pasteboard did not take the text".into()));
        }
        Ok(())
    }

    fn clear(&self) -> Result<()> {
        self.board().clearContents();
        Ok(())
    }
}

/// Swift `ClipboardShortcut`: an unmodified ⌘C or ⌘V, recognized without depending on an input method's Korean characters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClipboardShortcut {
    Copy,
    Paste,
}

impl ClipboardShortcut {
    /// Swift `ClipboardShortcut.match`: `modifiers` is `NSEvent.modifierFlags`' raw value, `characters` the event's
    /// `charactersIgnoringModifiers`, `key_code` its virtual key (C is 8, V is 9). Command alone (caps lock, the numeric pad and the
    /// function key do not count); a printable Latin layout keeps its own mapping, so Dvorak's letters at the C/V positions are not
    /// shadowed.
    pub fn of(modifiers: u64, characters: Option<&str>, key_code: u16) -> Option<ClipboardShortcut> {
        let meaningful = modifiers & 0xFFFF_0000 & !((1u64 << 16) | (1 << 21) | (1 << 23));
        if meaningful != 1 << 20 {
            return None;
        }
        let characters = characters.unwrap_or("").to_lowercase();
        if characters == "c" {
            return Some(ClipboardShortcut::Copy);
        }
        if characters == "v" {
            return Some(ClipboardShortcut::Paste);
        }
        if characters.chars().any(|c| (0x20..=0x024F).contains(&(c as u32)) || (0x1E00..=0x1EFF).contains(&(c as u32))) {
            return None;
        }
        match key_code {
            8 => Some(ClipboardShortcut::Copy),
            9 => Some(ClipboardShortcut::Paste),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const COMMAND: u64 = 1 << 20;

    /// Swift `ClipboardShortcutTests.swift:7 "koreanAndMissingCharactersUsePhysicalKey"`.
    #[test]
    fn korean_and_missing_characters_use_physical_key() {
        assert_eq!(ClipboardShortcut::of(COMMAND, Some("ㅊ"), 8), Some(ClipboardShortcut::Copy));
        assert_eq!(ClipboardShortcut::of(COMMAND, Some("ㅍ"), 9), Some(ClipboardShortcut::Paste));
        assert_eq!(ClipboardShortcut::of(COMMAND, None, 8), Some(ClipboardShortcut::Copy));
        assert_eq!(ClipboardShortcut::of(COMMAND, Some(""), 9), Some(ClipboardShortcut::Paste));
        assert_eq!(ClipboardShortcut::of(COMMAND, Some("ㅁ"), 0), None);
    }

    /// Swift `ClipboardShortcutTests.swift:15 "characterMappingWinsOverPhysicalPosition"`.
    #[test]
    fn character_mapping_wins_over_physical_position() {
        assert_eq!(ClipboardShortcut::of(COMMAND, Some("v"), 8), Some(ClipboardShortcut::Paste));
        assert_eq!(ClipboardShortcut::of(COMMAND, Some("C"), 9), Some(ClipboardShortcut::Copy));
        assert_eq!(ClipboardShortcut::of(COMMAND, Some("c"), 42), Some(ClipboardShortcut::Copy));
        for text in ["j", ".", "é"] {
            assert_eq!(ClipboardShortcut::of(COMMAND, Some(text), 8), None, "{text}");
            assert_eq!(ClipboardShortcut::of(COMMAND, Some(text), 9), None, "{text}");
        }
    }

    /// Swift `ClipboardShortcutTests.swift:25 "extraModifiersKeepTheirOwnShortcuts"`.
    #[test]
    fn extra_modifiers_keep_their_own_shortcuts() {
        for extra in [1u64 << 17, 1 << 18, 1 << 19] {
            assert_eq!(ClipboardShortcut::of(COMMAND | extra, Some("ㅊ"), 8), None);
            assert_eq!(ClipboardShortcut::of(COMMAND | extra, Some("ㅍ"), 9), None);
        }
        assert_eq!(ClipboardShortcut::of(0, Some("c"), 8), None);
        assert_eq!(ClipboardShortcut::of(COMMAND | 1 << 16 | 1 << 21 | 1 << 23, Some("ㅍ"), 9), Some(ClipboardShortcut::Paste));
    }

    /// The pasteboard round trip on a pasteboard of the test's own (never the user's): text in and out, a secret readable here and
    /// marked concealed, and a clear that leaves no text.
    #[test]
    fn text_goes_round_a_named_pasteboard_and_a_secret_is_concealed() {
        let clipboard = MacClipboard::named(&format!("dev.mightycode.tests.{}", std::process::id()));
        clipboard.write_text("한국어 clipboard").unwrap();
        assert_eq!(clipboard.read_text().unwrap().as_deref(), Some("한국어 clipboard"));
        clipboard.write_secret("connection-key").unwrap();
        assert_eq!(clipboard.read_text().unwrap().as_deref(), Some("connection-key"));
        let types: Vec<String> = clipboard.board().types().map(|types| types.iter().map(|t| t.to_string()).collect()).unwrap_or_default();
        assert!(types.iter().any(|t| t == CONCEALED) && types.iter().any(|t| t == TRANSIENT), "{types:?}");
        clipboard.clear().unwrap();
        assert_eq!(clipboard.read_text().unwrap(), None);
        clipboard.release();
    }
}
