//! M-G: the page's own picture for the help capture and the smoke (`WKWebView takeSnapshotWithConfiguration:`, with a rectangle for
//! an element's box), where Windows uses WebView2's `CapturePreview` and the DevTools protocol (src-tauri `help_capture.rs` through
//! `platform.rs`). Swift `HelpCaptureDiagnostics.swift` (`cacheDisplay` of the SwiftUI window).

/// The page's picture as PNG bytes: the whole page, or `clip` (x, y, width, height in CSS pixels) only (Unsupported until M-G).
pub async fn snapshot(_page: &tauri::Webview, _clip: Option<(f64, f64, f64, f64)>) -> Result<Vec<u8>, String> {
    Err("the page's picture is not available on this platform yet (M-G)".into())
}
