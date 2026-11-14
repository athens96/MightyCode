//! 화면 보기·조작 (C# `BuildScreenShareSection` → `BuildScreenSharePaused`), moved as it was from `later.rs` in W8 Step 0. Owner: W8-F
//! (it stays the paused line while `screen::ENABLED` is false).

use std::sync::Arc;

use mighty_core::design_tokens::Token;
use mighty_native::kit::TextStyle;
use mighty_native::ui::{El, Ui};

use crate::host::Host;

/// 화면 보기·조작 (C# `BuildScreenShareSection` → `BuildScreenSharePaused`, MainWindow.ScreenShare.cs:179-185): screen view is off on
/// Windows (`ScreenShareAvailability.Enabled` is false), so the box says it is paused.
pub fn screen_share(_host: &Arc<Host>, ui: &mut Ui) -> El {
    let mut rows = ui.settings_rows();
    rows.add(ui.settings_text(ui.locale().get("settings.screenShare.paused"), TextStyle::size(11.0).ink(Token::Ink2)));
    rows.finish().aid("settings-screen-share-paused")
}
