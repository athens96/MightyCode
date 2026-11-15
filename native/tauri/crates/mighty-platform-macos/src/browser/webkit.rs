//! M-E: a browser pane as a WKWebView child webview (Tauri's `add_child`), with `data_store_identifier` = a UUID-v5 of the workspace's
//! profile key (macOS 14+), popups refused, downloads cancelled, its snapshot from `takeSnapshotWithConfiguration`, its navigations
//! through `BrowserListener`; `browser{status:"ready"}` at once (there is no runtime to install).

use mighty_platform::{BrowserOptions, BrowserSurface, PlatformError, Result};

pub async fn open(_window: &tauri::Window, _options: BrowserOptions) -> Result<Box<dyn BrowserSurface>> {
    Err(PlatformError::unsupported("SurfaceFactory", "open_browser"))
}
