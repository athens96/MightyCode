//! M-A: the pasteboard (`NSPasteboard.generalPasteboard` text: read, write, clear). Swift `ClipboardShortcut.swift` (26 lines) and the
//! NSPasteboard uses; contract `mighty_platform::clipboard`. Ghostty's own copy and paste go through M-D's view; this is the host's
//! (the composer's and the files pane's).

use std::sync::Arc;

use mighty_platform::Clipboard;
use mighty_platform::unsupported::Unsupported;

/// The Mac's `Clipboard` (Unsupported until M-A).
pub fn clipboard() -> Arc<dyn Clipboard> {
    Arc::new(Unsupported)
}
