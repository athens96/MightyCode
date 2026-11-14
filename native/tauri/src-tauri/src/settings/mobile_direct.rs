//! The direct path's rows of 모바일 원격 (C# MainWindow.MobileDirect.cs, `BuildMobileRemoteSection`'s direct part), moved as they were
//! from `later.rs` in W8 Step 0: the switch, the port, the status and the firewall rule. Owner: W8-C.

use mighty_core::design_tokens::{Token, metrics};
use mighty_core::model::MobileRemoteSettings;
use mighty_native::kit::{ControlSize, TextStyle};
use mighty_native::ui::{Border, El, Kind, Thickness, Ui, VAlign};

use super::mobile::MOBILE_INDENT;

/// `MobileDirectPortWidth` (the remote workspaces box's port field takes it too).
pub(super) const MOBILE_PORT_WIDTH: f64 = 80.0;

/// The direct rows as C# draws them with no host: every control disabled, the firewall rule unknown.
pub fn rows(ui: &mut Ui, saved: &MobileRemoteSettings) -> El {
    let locale = ui.locale();
    let ink3 = Some(ui.brush(Token::Ink3));
    let small = |ui: &Ui, key: &str| ui.settings_push(ui.button(locale.get(key), || {}), ControlSize::Small, false, false);
    let caption = |ui: &Ui, text: String| ui.settings_text(text, TextStyle::size(11.0).ink(Token::Ink2));
    let direct_label = locale.get("mobileRemote.direct.toggle");
    let direct = ui.settings_switch(&direct_label, saved.direct_enabled, Some("settings-mobile-direct-toggle"), |_| {}).enabled(false);
    let direct_line =
        ui.settings_labeled(ui.settings_titled(&direct_label, locale.get("mobileRemote.direct.windows.description")), direct, true);
    let port_label = locale.get("mobileRemote.direct.portLabel");
    let port = ui
        .settings_field(El::text_box(saved.direct_port.to_string()), 11.0)
        .width(MOBILE_PORT_WIDTH)
        .named(&port_label)
        .aid("settings-mobile-direct-port")
        .enabled(false);
    let apply_port = small(ui, "settings.mobileRemote.applyButton").aid("settings-mobile-direct-apply").enabled(false);
    let indent = Thickness::new(MOBILE_INDENT, 0.0, 0.0, 0.0);
    let port_row = El::border(Border {
        child: Some(Box::new(El::stack(
            true,
            metrics::SPACING_SM,
            vec![ui.settings_text(&port_label, TextStyle::default()), port, apply_port],
        ))),
        ..Border::default()
    })
    .margin(indent)
    .visible(saved.direct_enabled);
    let status_row = El::border(Border {
        child: Some(Box::new(El::stack(
            true,
            metrics::SPACING_SM,
            vec![
                El::new(Kind::Ellipse { fill: ink3 }).width(8.0).height(8.0).va(VAlign::Center),
                ui.settings_text("", TextStyle::size(11.0).ink(Token::Ink2).selectable()).aid("settings-mobile-direct-status"),
            ],
        ))),
        ..Border::default()
    })
    .margin(indent)
    .visible(saved.enabled);
    let firewall_words = El::stack(
        false,
        metrics::SPACING_XXS,
        vec![
            caption(ui, locale.get("mobileRemote.direct.windows.firewall.unknown")).aid("settings-mobile-firewall-state"),
            caption(ui, locale.get("mobileRemote.direct.windows.firewall.description")),
            caption(ui, locale.get("mobileRemote.direct.windows.portHint")),
            caption(ui, locale.get("remote.windows.firewall.removeClosesBoth")),
        ],
    );
    let firewall_buttons = El::stack(
        true,
        metrics::SPACING_SM,
        vec![
            small(ui, "remote.windows.firewall.update").aid("settings-mobile-firewall-update").visible(false).enabled(false),
            small(ui, "setup.button.recheck").aid("settings-mobile-firewall-button").enabled(false),
        ],
    );
    let firewall_row =
        El::border(Border { child: Some(Box::new(ui.settings_labeled(firewall_words, firewall_buttons, true))), ..Border::default() })
            .margin(indent)
            .visible(saved.direct_enabled);
    El::stack(false, metrics::SPACING_SM, vec![direct_line, port_row, status_row, firewall_row])
}
