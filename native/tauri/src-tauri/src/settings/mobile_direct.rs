//! The direct path's rows of 모바일 원격 (C# MainWindow.MobileRemote.cs `BuildMobileRemoteSection`'s direct part and `ShowDirect`; the
//! firewall row's state from MainWindow.MobileDirect.cs): the Tailscale switch over the Windows words, and while it is on its port with
//! its own apply button, what the path found, and the Windows Firewall rule's row with the one button that asks Windows to elevate
//! (add while there is none, remove while there is one, read again while it could not be read) and the update button. The switch and
//! the port take effect as they are applied (`direct::write`); the row says only Core's words. In a smoke every control is off, as C#'s
//! Loaded handler leaves them. Owner: W8-C.

use std::sync::Arc;

use mighty_core::design_tokens::{Token, metrics};
use mighty_core::model::MobileRemoteSettings;
use mighty_native::kit::{ControlSize, TextStyle};
use mighty_native::ui::{Border, El, Kind, Thickness, Ui, VAlign};
use mighty_platform::FirewallRule;
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
    // The rule is read (unelevated) once each time Settings shows the box, unless a change still waits on its prompt (C#'s Loaded).
    let read = ui.local("mobile-direct-firewall-read", || false);
    if !read.get() {
        read.set(true);
        direct::settings_opened(host);
    }
    let firewall = direct::firewall_view(host);
    let ports = direct::firewall_ports(host);
    let status = direct::status(host);
    let words = strings::status_detail(&locale, &status);
    let caption = |ui: &Ui, text: String| ui.settings_text(text, TextStyle::size(11.0).ink(Token::Ink2));
    let small = |ui: &Ui, title: String, press: Box<dyn Fn() + Send + Sync>| {
        ui.settings_push(ui.button(title, press), ControlSize::Small, false, false)
    };

    let direct_label = locale.get("mobileRemote.direct.toggle");
    let to = host.clone();
    let switch = ui
        .settings_switch(&direct_label, saved.direct_enabled, Some("settings-mobile-direct-toggle"), move |on| {
            direct::switch_pressed(&to, on)
        })
        .enabled(!smoke);
    let direct_line = ui.settings_labeled(ui.settings_titled(&direct_label, strings::description(&locale)), switch, true);

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

    let present = matches!(firewall.rule, FirewallRule::Present { .. });
    let state_words = format!(
        "{}{}{}",
        strings::firewall_state(&locale, &firewall.rule),
        direct::firewall_needs_update_words(host),
        firewall.message.as_ref().map(|message| format!(" · {message}")).unwrap_or_default()
    );
    let firewall_words = El::stack(
        false,
        metrics::SPACING_XXS,
        vec![
            caption(ui, state_words).aid("settings-mobile-firewall-state"),
            caption(ui, strings::firewall_description(&locale)),
            caption(ui, strings::port_hint(&locale)),
            caption(ui, locale.get("remote.windows.firewall.removeClosesBoth")),
        ],
    );
    let unknown = firewall.rule == FirewallRule::Unreadable;
    let label = if unknown { strings::firewall_recheck_button(&locale) } else { strings::firewall_button(&locale, &firewall.rule) };
    let free = !smoke && !firewall.busy;
    let to = host.clone();
    let button = small(ui, label.clone(), Box::new(move || direct::firewall_pressed(&to)))
        .named(&label)
        .aid("settings-mobile-firewall-button")
        // With no port to open and no rule to remove there is nothing to press, so the button is not shown at all.
        .visible(firewall.rule != FirewallRule::Missing || !ports.is_empty())
        // Adding or removing elevates: only with --allow-state-write (D4); reading again never does.
        .enabled(free && (unknown || firewall.may_change));
    let to = host.clone();
    let update = small(ui, locale.get("remote.windows.firewall.update"), Box::new(move || direct::firewall_update_pressed(&to)))
        .aid("settings-mobile-firewall-update")
        .visible(present && !ports.is_empty())
        .enabled(free && firewall.may_change);
    let firewall_buttons = El::stack(true, metrics::SPACING_SM, vec![update, button]);
    let firewall_row =
        El::border(Border { child: Some(Box::new(ui.settings_labeled(firewall_words, firewall_buttons, true))), ..Border::default() })
            .margin(indent)
            .visible(saved.direct_enabled || present);
    El::stack(false, metrics::SPACING_SM, vec![direct_line, port_row, status_row, firewall_row])
}
