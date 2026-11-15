//! M-F: deep links on the Mac (`RunEvent::Opened`, the bundle's `CFBundleURLTypes`): `mightycode://open?path=<abs>&prompt=<≤64 KB>&
//! agent=<provider>`, parsed strictly (Swift `OSIntegration.swift:7-34`); both `mightycode` and the legacy `mightyclaude` accepted
//! (plan D11, where Swift dropped the legacy one at run time). The handler waits up to 10 s for the page, asks before adding a new
//! folder, and puts the prompt in the draft without sending it (`AppStore+OSIntegration.swift:9-43`); a cold start's address arrives
//! after the window is ready.

use std::sync::Arc;

use tauri::Manager;

use crate::host::Host;

/// Addresses the OS opened the app with. Step 0: logged (their count only: a prompt is the user's text) and left for M-F.
#[cfg_attr(not(target_os = "macos"), allow(dead_code, reason = "RunEvent::Opened is the Mac's (and iOS's); Windows has no such event"))]
pub fn opened(app: &tauri::AppHandle, urls: &[tauri::Url]) {
    if let Some(host) = app.try_state::<Arc<Host>>() {
        host.log.line(&format!("deep link: {} address(es) received; opening them is the Mac stage's (M-F)", urls.len()));
    }
}
