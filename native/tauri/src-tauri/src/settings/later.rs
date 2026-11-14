//! The styles box, host-absent as in C# (MainWindow.Styles.cs) until W9. Owner: W6-A. (W8 Step 0 moved the other later-stage boxes to
//! a file each: `mobile.rs`, `mobile_direct.rs`, `remote.rs`, `companion.rs`, `screen.rs`.)

use std::sync::Arc;

use mighty_core::design_tokens::{Token, metrics};
use mighty_native::kit::{ControlSize, TextStyle};
use mighty_native::ui::{El, Ui};

use crate::host::Host;

/// 스타일 (C# `BuildStylesSection`, MainWindow.Styles.cs:18-139): the register and rescan buttons, the description, then each registered
/// style's row. Styles (their registry, trust store and approval sheet) are W9's: until then no style is registered, so the list is
/// empty, register's file picker and rescan have nothing to read (both shown disabled), and nothing is locked.
pub fn styles(_host: &Arc<Host>, ui: &mut Ui) -> El {
    let locale = ui.locale();
    let mut rows = ui.settings_rows();
    let register = ui
        .settings_push(ui.button(locale.get("settings.styles.registerButton"), || {}).enabled(false), ControlSize::Regular, false, false)
        .aid("settings-style-register");
    let rescan = ui
        .settings_push(ui.button(locale.get("settings.styles.rescanButton"), || {}).enabled(false), ControlSize::Regular, false, false)
        .aid("settings-style-rescan");
    rows.add(El::stack(true, metrics::SPACING_SM, vec![register, rescan]));
    rows.add(ui.settings_text(locale.get("settings.styles.description"), TextStyle::size(11.0).ink(Token::Ink2)));
    rows.finish().aid("settings-style-rows")
}
