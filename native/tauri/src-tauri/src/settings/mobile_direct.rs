//! The direct path's rows of 원격 연결's 휴대폰 tab (C# MainWindow.MobileRemote.cs `BuildMobileRemoteSection`'s direct part and
//! `ShowDirect`): the Tailscale switch over this OS's words, and while it is on its port with its own apply button and what the path
//! found. The Windows Firewall rule it needs is the one sharing uses too, drawn once under the tab with one set of words
//! (remote.rs `firewall`). The switch and the port take effect as they are applied (`direct::write`); the rows say only Core's words.
//! In a smoke every control is off, as C#'s Loaded handler leaves them. Owner: W8-C.

use std::sync::Arc;

use mighty_core::design_tokens::{Token, metrics};
use mighty_core::model::MobileRemoteSettings;
use mighty_native::kit::{ControlSize, TextStyle};
use mighty_native::ui::{Border, El, Kind, Thickness, Ui, VAlign};
use mighty_remote::direct::strings;

use super::mobile::MOBILE_INDENT;
use crate::direct;
use crate::host::Host;

/// `MobileDirectPortWidth` (the remote workspaces box's port field takes it too).
pub(super) const MOBILE_PORT_WIDTH: f64 = 80.0;

/// The direct rows (C#'s `directBlock`, shown by `ShowDirect`).
pub fn rows(host: &Arc<Host>, ui: &mut Ui, saved: &MobileRemoteSettings) -> El {
    let locale = ui.locale();
    let smoke = host.smoke.is_some();
    let status = direct::status(host);
    let words = strings::status_detail(&locale, &status);
    let small = |ui: &Ui, title: String, press: Box<dyn Fn() + Send + Sync>| {
        ui.settings_push(ui.button(title, press), ControlSize::Small, false, false)
    };

    // The Mac (no firewall step, `PlatformProfile::direct_connection_firewall`) says this Mac's own words.
    let has_firewall = direct::has_firewall(host);
    let description = if has_firewall { strings::description(&locale) } else { locale.get("mobileRemote.direct.description") };
    let direct_label = locale.get("mobileRemote.direct.toggle");
    let to = host.clone();
    let switch = ui
        .settings_switch(&direct_label, saved.direct_enabled, Some("settings-mobile-direct-toggle"), move |on| {
            direct::switch_pressed(&to, on)
        })
        .enabled(!smoke);
    let direct_line = ui.settings_labeled(ui.settings_titled(&direct_label, description), switch, true);

    let port_label = locale.get("mobileRemote.direct.portLabel");
    let typed = ui.local("mobile-direct-port", || saved.direct_port.to_string());
    let into = typed.clone();
    let port = ui
        .settings_field(El::text_box(typed.get()).on_text(move |text| into.set(text)), 11.0)
        .width(MOBILE_PORT_WIDTH)
        .named(&port_label)
        .aid("settings-mobile-direct-port")
        .enabled(!smoke);
    let applies = !direct::writing() && !smoke && direct::typed_port(&typed.get()).is_some_and(|port| port != saved.direct_port);
    let (to, apply_typed) = (host.clone(), typed.clone());
    let apply_port =
        small(ui, locale.get("settings.mobileRemote.applyButton"), Box::new(move || direct::apply_port(&to, &apply_typed.get())))
            .aid("settings-mobile-direct-apply")
            .enabled(applies);
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

    let dot = Some(ui.brush(if status.listening() { Token::Done } else { Token::Ink3 }));
    let status_row = El::border(Border {
        child: Some(Box::new(El::stack(
            true,
            metrics::SPACING_SM,
            vec![
                El::new(Kind::Ellipse { fill: dot }).width(8.0).height(8.0).va(VAlign::Center),
                ui.settings_text(words.clone(), TextStyle::size(11.0).ink(Token::Ink2).selectable()).aid("settings-mobile-direct-status"),
            ],
        ))),
        ..Border::default()
    })
    .margin(indent)
    .visible(saved.enabled && saved.direct_enabled && !words.is_empty());

    El::stack(false, metrics::SPACING_SM, vec![direct_line, port_row, status_row])
}
