//! The terminal's clipboard rules with no AppKit or libghostty in them (the vendored `TerminalController+Callbacks.swift` and Mighty's
//! `LocalTerminalSession.terminalDidRequestClipboardConfirmation`), so they are tested as they are; `runtime`'s callbacks apply them.
//! The numbers are `ghostty.h`'s.

/// `GHOSTTY_CLIPBOARD_STANDARD`.
pub const STANDARD: i32 = 0;
/// `GHOSTTY_CLIPBOARD_REQUEST_PASTE` … `GHOSTTY_CLIPBOARD_REQUEST_LIST`.
pub const REQUEST_PASTE: i32 = 0;
pub const REQUEST_OSC_52_READ: i32 = 1;
pub const REQUEST_OSC_52_WRITE: i32 = 2;
pub const REQUEST_KITTY_READ: i32 = 3;
pub const REQUEST_LIST: i32 = 5;

/// Whether libghostty's confirmation of a pending read may let it through: only the user's own paste; a program's read (OSC 52,
/// kitty's protocol, a list) is refused, as Swift answered it (`request.respond(allow: request.kind == .paste)`).
pub fn allow_confirm(request: i32) -> bool {
    request == REQUEST_PASTE
}

/// Whether a clipboard write lands: the standard clipboard only (the selection clipboard is advertised so copy-on-select writes go
/// nowhere), and only a write that needs no confirmation (a copy binding); `clipboard-write = ask` makes a program's OSC 52 write ask,
/// and the answer is no.
pub fn accept_write(clipboard: i32, confirm: bool) -> bool {
    clipboard == STANDARD && !confirm
}

/// `text/plain` with or without parameters.
pub fn is_text_mime(mime: &str) -> bool {
    mime == "text/plain" || mime.starts_with("text/plain;")
}

/// What a clipboard read is answered with (`readClipboard`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Read {
    /// Nothing this terminal can serve (it asked only for other kinds than text).
    Unsupported,
    /// A list request: no data, only the kinds there are.
    List { available: Vec<&'static str> },
    /// The clipboard holds no text.
    Unavailable,
    /// The clipboard's text, and the kinds listed with it when the request lists them.
    Text { available: Vec<&'static str> },
}

/// `readClipboard`'s decision from what was asked (`mimes`, whether the kinds there are should be listed) and whether the clipboard
/// holds text.
pub fn read(mimes: &[String], list_available: bool, has_text: bool) -> Read {
    let list_only = mimes.is_empty() && list_available;
    if !list_only && !mimes.is_empty() && !mimes.iter().any(|mime| is_text_mime(mime)) {
        return Read::Unsupported;
    }
    let available = if list_available && has_text { vec!["text/plain"] } else { Vec::new() };
    if list_only {
        return Read::List { available };
    }
    if has_text { Read::Text { available } } else { Read::Unavailable }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Swift `terminalDidRequestClipboardConfirmation`: a paste is let through, a program's read of any kind is not.
    #[test]
    fn only_a_paste_is_confirmed() {
        assert!(allow_confirm(REQUEST_PASTE));
        for request in [REQUEST_OSC_52_READ, REQUEST_OSC_52_WRITE, REQUEST_KITTY_READ, 4, REQUEST_LIST, 99] {
            assert!(!allow_confirm(request), "{request}");
        }
    }

    /// A copy binding writes the standard clipboard; a program's write asked to be confirmed lands nowhere, nor does the selection
    /// clipboard.
    #[test]
    fn only_an_unconfirmed_standard_write_lands() {
        assert!(accept_write(STANDARD, false));
        assert!(!accept_write(STANDARD, true), "OSC 52 under clipboard-write = ask");
        assert!(!accept_write(1, false) && !accept_write(2, false), "selection and primary");
    }

    /// `readClipboard`: a list request reads no data; a request only for other kinds is unsupported; an empty clipboard is unavailable;
    /// text is served, its kind listed only when asked.
    #[test]
    fn reads_follow_swifts_reader() {
        let text = |mime: &str| vec![mime.to_string()];
        assert_eq!(read(&[], true, true), Read::List { available: vec!["text/plain"] });
        assert_eq!(read(&[], true, false), Read::List { available: vec![] });
        assert_eq!(read(&text("image/png"), false, true), Read::Unsupported);
        assert_eq!(read(&text("text/plain;charset=utf-8"), false, true), Read::Text { available: vec![] });
        assert_eq!(read(&[], false, true), Read::Text { available: vec![] });
        assert_eq!(read(&text("text/plain"), true, true), Read::Text { available: vec!["text/plain"] });
        assert_eq!(read(&text("text/plain"), false, false), Read::Unavailable);
        assert!(is_text_mime("text/plain") && is_text_mime("text/plain; charset=utf-8") && !is_text_mime("text/plainx"));
    }
}
