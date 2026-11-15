//! M-E: browser panes. With `browser.engineEnabled` off (plan D4) a pane is WKWebView, a Tauri child webview like Windows' WebView2
//! panes, with its workspace's own data store ([`webkit`]); on and bundled, CEF through the Swift app's own C bridge ([`cef`],
//! reached by `dlopen`, with CEF's methods added to tao's NSApplication class by [`app_class`]). Swift `CefBrowserEngine.swift` (196
//! lines), `CefBrowserRuntime.swift` (174), `BrowserPaneView.swift` (124), `MightyApplication.swift`, `BrowserBridge/cef_bridge.m`
//! (364), `helper.c` (35), `BrowserEngine.swift`, `BrowserEngine.lock`. The Step 0 spikes `native/tauri/spikes/mac/webkit` and
//! `native/tauri/spikes/mac/cef` are the proof and the starting point.

pub mod app_class;
pub mod cef;
pub mod webkit;

use mighty_platform::{BrowserOptions, BrowserSurface, Result};

/// Opens a browser pane over `window`'s page: CEF when the engine is on and bundled, else WKWebView (Unsupported until M-E).
pub async fn open(window: &tauri::Window, options: BrowserOptions) -> Result<Box<dyn BrowserSurface>> {
    webkit::open(window, options).await
}
