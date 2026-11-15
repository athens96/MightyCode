//! M-F: the menu's input method items (plan D18). "Reconnect Input Method" re-activates the app, waits for the key window and makes
//! the page's webview first responder again (the AppKit half of Swift `InputSessionRecoveryCoordinator.swift`); "Save Input Method
//! Diagnostics" writes `<data>/diagnostics/ime-<stamp>-<id>.json` (folder 0700, file 0600) with the current TIS input sources
//! (`InputMethodDiagnostics.swift:234-300`). `HangulComposer` is dead code and not ported (plan fact 13).

use std::path::{Path, PathBuf};

use mighty_platform::{PlatformError, Result};

/// The page's webview takes the keyboard back (Unsupported until M-F).
pub fn reconnect() -> Result<()> {
    Err(PlatformError::unsupported("InputMethod", "reconnect"))
}

/// The diagnostics file under `data_dir`; its path (Unsupported until M-F).
pub fn save_diagnostics(_data_dir: &Path) -> Result<PathBuf> {
    Err(PlatformError::unsupported("InputMethod", "save_diagnostics"))
}
