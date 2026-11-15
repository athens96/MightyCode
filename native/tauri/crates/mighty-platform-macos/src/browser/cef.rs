//! M-E: a browser pane in CEF: `Contents/Frameworks/MightyCEFBridge.dylib` (cef_bridge.m built unchanged) by `dlopen`, `no_sandbox`,
//! `MightyCode Helper.app` as the subprocess, a per-pane request context whose cache is `<data>/browser-profiles/<workspaceId>` (so
//! 0.4.x sign-ins survive), a 60 Hz `mighty_cef_work` in the common run loop modes, `mighty_cef_shutdown` before the app exits.
//! CEF is initialized before tao's run loop starts (the bridge requires it): after `tauri::Builder::build`, before `App::run`.

use mighty_platform::{BrowserOptions, BrowserSurface, PlatformError, Result};

pub async fn open(_window: &tauri::Window, _options: BrowserOptions) -> Result<Box<dyn BrowserSurface>> {
    Err(PlatformError::unsupported("SurfaceFactory", "open_browser (CEF)"))
}
