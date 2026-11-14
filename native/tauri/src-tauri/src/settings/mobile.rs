//! 모바일 원격 (C# `BuildMobileRemoteSection`, MainWindow.MobileRemote.cs:93-456), moved as it was from `later.rs` in W8 Step 0. Owner:
//! W8-A (the relay rows, the QR, the device list, rotate with its confirmation, the masked key with show and copy, the relay field;
//! the direct rows are W8-C's `mobile_direct.rs`).

use std::sync::Arc;

use mighty_core::design_tokens::{Token, metrics};
use mighty_native::kit::{ControlSize, TextStyle};
use mighty_native::ui::{El, Grid, Kind, Thickness, Track, Ui, VAlign};

use crate::host::Host;

/// `MobileRelayHost.DefaultRelay`.
const DEFAULT_RELAY: &str = "wss://mighty-relay.athens96.workers.dev";
/// `MobileQrSize`, `MobileRemoteIndent` (`Spacing.Xl`).
const MOBILE_QR_SIZE: f64 = 200.0;
pub(super) const MOBILE_INDENT: f64 = metrics::SPACING_XL;

/// C#'s local `Heading`: a box's sub-heading, 11pt medium `ink2`.
fn heading(ui: &Ui, title: String) -> El {
    ui.settings_text(title, TextStyle::size(11.0).ink(Token::Ink2).medium())
}

/// 모바일 원격 (C# `BuildMobileRemoteSection`, MainWindow.MobileRemote.cs:93-456) as C# draws it before the phone remote exists (W8)
/// — what its smoke draws with no host: the saved settings shown with every switch, field and button disabled (C#'s
/// `ShowControls` and its Loaded handler), the state off, the firewall rule unknown (re-check), and the pairing rows (QR, host id, the
/// key as dots) and the device list hidden. A press changes nothing.
pub fn mobile_remote(host: &Arc<Host>, ui: &mut Ui) -> El {
    let locale = ui.locale();
    let saved = host.view().snapshot.mobile_remote.clone().unwrap_or_default();
    let mut rows = ui.settings_rows();
    let ink3 = Some(ui.brush(Token::Ink3));
    let small = |ui: &Ui, key: &str| ui.settings_push(ui.button(locale.get(key), || {}), ControlSize::Small, false, false);
    let caption = |ui: &Ui, text: String| ui.settings_text(text, TextStyle::size(11.0).ink(Token::Ink2));

    let allow = locale.get("windows.mobile.allow");
    let switch = ui.settings_switch(&allow, saved.enabled, Some("settings-mobile-toggle"), |_| {}).enabled(false);
    rows.add(ui.settings_labeled(ui.settings_titled(&allow, locale.get("windows.mobile.relayDescription")), switch, true));

    let state = El::stack(
        true,
        metrics::SPACING_SM,
        vec![
            El::new(Kind::Ellipse { fill: ink3 }).width(8.0).height(8.0).va(VAlign::Center),
            caption(ui, locale.get("settings.mobileRemote.statusOff")),
        ],
    );
    let words = El::stack(
        false,
        metrics::SPACING_XXS,
        vec![state, caption(ui, locale.get("mobileRemote.status.off")).aid("settings-mobile-detail")],
    );
    let reconnect = small(ui, "settings.mobileRemote.reconnectButton").aid("settings-mobile-reconnect").visible(false).enabled(false);
    rows.add(ui.settings_labeled(words, reconnect, true));

    rows.add(heading(ui, locale.get("windows.mobile.connectionTitle")));
    let relay = ui
        .settings_field(El::text_box(saved.relay_url.clone().unwrap_or_default()), 11.0)
        .placeholder(DEFAULT_RELAY)
        .named(locale.get("settings.mobileRemote.relayLabel"))
        .aid("settings-mobile-relay")
        .enabled(false);
    let apply = ui
        .settings_push(ui.button(locale.get("settings.mobileRemote.applyButton"), || {}), ControlSize::Regular, false, false)
        .aid("settings-mobile-apply")
        .enabled(false)
        .cell(0, 1);
    let line = El::grid(Grid {
        children: vec![relay, apply],
        cols: vec![Track::Star(1.0), Track::Auto],
        col_spacing: metrics::SPACING_SM,
        ..Grid::default()
    })
    .margin(Thickness::new(MOBILE_INDENT, 0.0, 0.0, 0.0));
    let default = caption(ui, locale.get_with("windows.mobile.relayDefaultTemplate", &[("address", DEFAULT_RELAY)]))
        .margin(Thickness::new(MOBILE_INDENT, 0.0, 0.0, 0.0));
    rows.add(El::stack(
        false,
        metrics::SPACING_SM,
        vec![ui.settings_titled(locale.get("windows.mobile.relayTitle"), locale.get("windows.mobile.relayExplanation")), line, default],
    ));

    // The direct path's rows (mobile_direct.rs, W8-C).
    rows.add(super::mobile_direct::rows(host, ui, &saved));

    // The pairing rows (title, QR, host id, key, copy and regenerate) and the devices' title: hidden without a host.
    rows.add(heading(ui, locale.get("windows.mobile.pairingTitle"))).visible = false;
    let qr = El::new(Kind::Image { src: None }).width(MOBILE_QR_SIZE).height(MOBILE_QR_SIZE).aid("settings-mobile-qr");
    rows.add(qr).visible = false;
    rows.add(heading(ui, locale.get("settings.mobileRemote.connectedDevicesTitle"))).visible = false;
    rows.add(heading(ui, locale.get("windows.mobile.advancedTitle")));
    let legacy_label = locale.get("settings.mobileRemote.legacyAppsToggle");
    let legacy = ui.settings_switch(&legacy_label, saved.allow_legacy_phones, Some("settings-mobile-legacy-toggle"), |_| {}).enabled(false);
    rows.add(ui.settings_labeled(
        ui.settings_titled(&legacy_label, locale.get("settings.mobileRemote.legacyAppsDescription")),
        legacy,
        true,
    ));
    rows.finish()
}
