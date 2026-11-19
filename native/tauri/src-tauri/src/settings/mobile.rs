//! 원격 연결's 휴대폰 tab, what was 모바일 원격 (C# `BuildMobileRemoteSection`, MainWindow.MobileRemote.cs:93-456). Owner: W8-A (the
//! relay rows, the QR, the device list, rotate with its confirmation, the masked key with show and copy, the relay field; the direct
//! rows are W8-C's `mobile_direct.rs`, the firewall rule is drawn once under the tab by remote.rs). The live box needs the relay host
//! (mobile.rs, `--allow-state-write`); without it the box is C#'s with no host.
//!
//! What the phone and computer pairing share lives here only: the QR, the host id and key, 키 다시 만들기 (it unpairs computers too)
//! and the relay's address, which sits folded under 고급 with the legacy-apps switch (the built-in relay serves everyone else).
//! Differences from C#: the QR is drawn at scale 1 (the page draws it in CSS pixels); the link is copied as a secret, as the share
//! tab copies it; the upload staging (W8-B's router) and screen share's sessions (W8-F, off) are not reset on a rotation here.

use std::sync::Arc;

use base64::Engine;
use mighty_core::design_tokens::{Token, metrics};
use mighty_core::model::MobileRemoteSettings;
use mighty_native::kit::{ControlSize, TextStyle};
use mighty_native::surface::{DefaultButton, DialogButton, DialogChrome, DialogResult, DialogSpec};
use mighty_native::ui::{Border, Content, El, Grid, HAlign, Kind, Local, Track, Ui, VAlign};
use mighty_remote::identity::MobileDeviceInfo;
use mighty_remote::relay::host::{RelayHost, normalize_relay};
use mighty_remote::relay::status::{self as relay_status, MobileRelayStatus};

use super::{caption, folded, sub_heading};
use crate::host::{Host, lock};

/// `MobileRelayHost.DefaultRelay`.
const DEFAULT_RELAY: &str = "wss://mighty-relay.athens96.workers.dev";
/// `MobileQrSize`, `MobileRemoteIndent` (`Spacing.Xl`).
const MOBILE_QR_SIZE: f64 = mighty_remote::identity::QR_SIZE;
/// `DesignMetrics.Radius.Row`: the QR's white and the marks' capsules.
const RADIUS_ROW: f64 = 8.0;
pub(super) const MOBILE_INDENT: f64 = metrics::SPACING_XL;
/// The pairing guide's steps (`settings.mobileRemote.guide.step1…3`): open the app, scan or paste, the phone lists below. The relay is
/// built in, so nothing is deployed first.
const GUIDE_STEPS: usize = 3;
/// A paired computer's symbol in the device list (Segoe's PC), where a phone has the phone's.
const COMPUTER_GLYPH: &str = "\u{e977}";

/// The relay's address field, its apply button and the default under it (folded under 고급): `live` when the relay host takes it.
fn relay_address(host: &Arc<Host>, ui: &Ui, saved: &MobileRemoteSettings, live: bool) -> El {
    let locale = ui.locale();
    let mobile = crate::mobile::ui(host);
    let saved_relay = saved.relay_url.clone().unwrap_or_default();
    let typed = if live { lock(&mobile.ui).typed.clone().unwrap_or_else(|| saved_relay.clone()) } else { saved_relay.clone() };
    let same =
        |address: &str| if address.is_empty() { String::new() } else { normalize_relay(address).unwrap_or_else(|| address.to_string()) };
    let typed_trimmed = typed.trim().to_string();
    let can_apply =
        live && same(&typed_trimmed) != same(&saved_relay) && (typed_trimmed.is_empty() || normalize_relay(&typed_trimmed).is_some());
    let typing = mobile.clone();
    let to = host.clone();
    let field = ui
        .settings_field(El::text_box(typed.clone()), 11.0)
        .placeholder(DEFAULT_RELAY)
        .named(locale.get("settings.mobileRemote.relayLabel"))
        .aid("settings-mobile-relay")
        .enabled(live)
        .on_text(move |words| {
            lock(&typing.ui).typed = Some(words);
            crate::mobile::touch(&to);
        });
    let (to, applying) = (host.clone(), mobile.clone());
    let apply = ui
        .settings_push(
            ui.button(locale.get("settings.mobileRemote.applyButton"), move || {
                let typed = lock(&applying.ui).typed.clone().unwrap_or_default().trim().to_string();
                let host = to.clone();
                let applying = applying.clone();
                crate::native::act(&to, async move {
                    if !typed.is_empty() && normalize_relay(&typed).is_none() {
                        return Err(host.locale.get("windows.mobile.invalidRelay"));
                    }
                    crate::mobile::write(&host, |saved| MobileRemoteSettings { relay_url: Some(typed.clone()), ..saved }).await?;
                    // The field then holds the address as it was saved.
                    lock(&applying.ui).typed = None;
                    crate::mobile::touch(&host);
                    Ok(())
                });
            }),
            ControlSize::Small,
            false,
            false,
        )
        .aid("settings-mobile-apply")
        .enabled(can_apply)
        .va(VAlign::Center)
        .cell(0, 1);
    let line = El::grid(Grid {
        children: vec![field, apply],
        cols: vec![Track::Star(1.0), Track::Auto],
        col_spacing: metrics::SPACING_SM,
        ..Grid::default()
    });
    let label = ui.settings_text(locale.get("settings.mobileRemote.relayLabel"), TextStyle::size(12.0));
    El::stack(
        false,
        metrics::SPACING_SM,
        vec![
            ui.settings_wide(label, line),
            caption(ui, locale.get_with("windows.mobile.relayDefaultTemplate", &[("address", DEFAULT_RELAY)])),
        ],
    )
}

/// The legacy-apps switch (folded under 고급): `live` when it takes effect.
fn legacy_apps(host: &Arc<Host>, ui: &Ui, saved: &MobileRemoteSettings, live: bool) -> El {
    let locale = ui.locale();
    let legacy_label = locale.get("settings.mobileRemote.legacyAppsToggle");
    let to = host.clone();
    let legacy = ui
        .settings_switch(&legacy_label, saved.allow_legacy_phones, Some("settings-mobile-legacy-toggle"), move |on| {
            if !live {
                return;
            }
            let host = to.clone();
            crate::native::act(&to, async move {
                crate::mobile::write(&host, |saved| MobileRemoteSettings { allow_legacy_phones: on, ..saved }).await
            });
        })
        .enabled(live);
    ui.settings_labeled(ui.settings_titled(&legacy_label, locale.get("settings.mobileRemote.legacyAppsDescription")), legacy, true)
}

/// 고급, folded: the relay's address and the legacy-apps switch, open the first time when either was changed from its default.
fn advanced(host: &Arc<Host>, ui: &mut Ui, saved: &MobileRemoteSettings, live: bool) -> El {
    let locale = ui.locale();
    let changed = saved.relay_url.as_deref().is_some_and(|url| !url.trim().is_empty()) || !saved.allow_legacy_phones;
    let body = El::stack(false, metrics::SPACING_MD, vec![relay_address(host, ui, saved, live), legacy_apps(host, ui, saved, live)]);
    folded(ui, "settings-mobile-advanced", changed, locale.get("settings.advancedTitle"), body)
}

/// 연결 방법: the relay (always, with the built-in address unless 고급 names another), then the Tailscale path (mobile_direct.rs).
fn connection(host: &Arc<Host>, ui: &mut Ui, saved: &MobileRemoteSettings) -> El {
    let locale = ui.locale();
    El::stack(
        false,
        metrics::SPACING_SM,
        vec![
            sub_heading(ui, locale.get("windows.mobile.connectionTitle")),
            ui.settings_titled(locale.get("windows.mobile.relayTitle"), locale.get("windows.mobile.relayExplanation")),
            super::mobile_direct::rows(host, ui, saved),
        ],
    )
}

/// 모바일 원격: the live box once the relay host exists (mobile.rs started it with `--allow-state-write`); before that — in a smoke, in
/// verification mode (said on a line of its own) — the box as C# draws it with no host.
pub fn mobile_remote(host: &Arc<Host>, ui: &mut Ui) -> El {
    match crate::mobile::relay(host) {
        Some(relay) if host.smoke.is_none() => live(host, ui, &relay),
        _ => at_rest(host, ui),
    }
}

/// C# `BuildMobileRemoteSection` with its host read (`Refresh`): the allow switch over its explanation; the state dot and words beside
/// the reconnect button over what the host is doing; 연결 방법 — the relay, then the direct rows (W8-C's); 휴대폰 연결 while the remote
/// is on — the QR beside the steps, the host id and the key (dots until shown) under their labels, the link and new-key buttons; the
/// paired phones and computers, a row each, whenever there is one (the phone switch on or off); 고급, folded — the relay's field with
/// its apply button and the default under it, and the legacy-apps switch. The switches take effect as they are switched and the apply
/// button is the address's own.
fn live(host: &Arc<Host>, ui: &mut Ui, relay: &Arc<RelayHost>) -> El {
    let locale = ui.locale();
    let saved = host.view().snapshot.mobile_remote.clone().unwrap_or_default();
    let mobile = crate::mobile::ui(host);
    let mut rows = ui.settings_rows();

    // A. The allow switch, and under it how the connection stands and what the host is doing.
    let allow = mighty_core::settings_words::text(&locale, crate::platform::words(), "windows.mobile.allow").unwrap_or_default();
    let to = host.clone();
    let switch = ui.settings_switch(&allow, saved.enabled, Some("settings-mobile-toggle"), move |on| {
        let host = to.clone();
        crate::native::act(&to, async move { crate::mobile::write(&host, |saved| MobileRemoteSettings { enabled: on, ..saved }).await });
    });
    rows.add(ui.settings_labeled(
        ui.settings_titled(
            &allow,
            mighty_core::settings_words::text(&locale, crate::platform::words(), "windows.mobile.relayDescription").unwrap_or_default(),
        ),
        switch,
        true,
    ));
    let status = relay.status();
    let dot = if !saved.enabled {
        Token::Ink3
    } else if status.connected {
        Token::Done
    } else {
        Token::WaitText
    };
    let words = if !saved.enabled {
        locale.get("settings.mobileRemote.statusOff")
    } else if !status.connected {
        locale.get("settings.mobileRemote.statusConnecting")
    } else if status.connections > 0 {
        locale.get_with("settings.mobileRemote.statusConnectedTemplate", &[("count", &status.connections.to_string())])
    } else {
        locale.get("settings.mobileRemote.statusRelayConnected")
    };
    let shown = if saved.enabled { status } else { MobileRelayStatus::disabled() };
    let state = El::stack(
        true,
        metrics::SPACING_SM,
        vec![El::new(Kind::Ellipse { fill: Some(ui.brush(dot)) }).width(8.0).height(8.0).va(VAlign::Center), caption(ui, words)],
    );
    let state_words = El::stack(
        false,
        metrics::SPACING_XXS,
        vec![state, caption(ui, relay_status::detail(&locale, &shown)).aid("settings-mobile-detail")],
    );
    let to = host.clone();
    let reconnect = ui
        .settings_push(
            ui.button(locale.get("settings.mobileRemote.reconnectButton"), move || {
                let host = to.clone();
                crate::native::act(&to, async move { crate::mobile::reconnect(&host).await });
            }),
            ControlSize::Small,
            false,
            false,
        )
        .aid("settings-mobile-reconnect")
        .visible(saved.enabled);
    rows.add(ui.settings_labeled(state_words, reconnect, true));

    // B. 연결 방법: the relay, then the Tailscale path.
    rows.add(connection(host, ui, &saved));

    // C. 휴대폰 연결, while the remote is on.
    let shows_identity = saved.enabled;
    let devices = relay.devices();
    if shows_identity {
        rows.add(El::stack(
            false,
            metrics::SPACING_SM,
            vec![sub_heading(ui, locale.get("windows.mobile.pairingTitle")), pairing(host, ui, relay, &mobile)],
        ));
    }
    // D. The paired phones and computers, a row each, whenever there is one: a computer paired through the relay (원격 연결's
    // 다른 컴퓨터와 페어링) is listed here too and must be seen and unpaired while the phone switch is off.
    if !devices.is_empty() {
        rows.add(sub_heading(ui, locale.get("settings.mobileRemote.connectedDevicesTitle")));
        for device in &devices {
            rows.add(device_row(host, ui, device)).key = Some(format!("mobile-device-{}", device.id));
        }
    }
    // E. 고급, folded: the relay's address and the legacy-apps switch.
    rows.add(advanced(host, ui, &saved, true));
    rows.finish()
}

/// The QR on white at radius 8 beside the steps, the host id and the key each under its label, and the link (copied as a secret, as
/// the share tab copies it) and key buttons.
fn pairing(host: &Arc<Host>, ui: &mut Ui, relay: &Arc<RelayHost>, mobile: &Arc<crate::mobile::MobileState>) -> El {
    let locale = ui.locale();
    let copy_note: Local<Option<bool>> = ui.local("mobile-copy-note", || None);
    let link = relay.pairing_url();
    let (qr_src, qr_side) = {
        let mut state = lock(&mobile.ui);
        if state.qr.as_ref().is_none_or(|(drawn, _, _)| *drawn != link)
            && let Some((png, side)) = mighty_remote::identity::pairing_qr(&link, 1.0)
        {
            let data = format!("data:image/png;base64,{}", base64::engine::general_purpose::STANDARD.encode(png));
            state.qr = Some((link.clone(), data, side as f64));
        }
        state.qr.as_ref().map(|(_, data, side)| (Some(data.clone()), *side)).unwrap_or((None, MOBILE_QR_SIZE))
    };
    let qr = El::new(Kind::Image { src: qr_src })
        .width(qr_side)
        .height(qr_side)
        .named(locale.get("settings.mobileRemote.qrAccessibility"))
        .aid("settings-mobile-qr");
    let qr = El::border(Border {
        child: Some(Box::new(qr)),
        background: Some(ui.brush(Token::OnStatus)),
        corner_radius: RADIUS_ROW,
        ..Border::default()
    })
    .va(VAlign::Top);

    // The steps: the number in a 14-wide column, right-aligned, 4 from its sentence.
    let steps: Vec<El> = (1..=GUIDE_STEPS)
        .map(|step| {
            let number = ui.settings_text(format!("{step}."), TextStyle::size(11.0).ink(Token::Ink2)).ha(HAlign::Right).va(VAlign::Top);
            let sentence = ui
                .settings_text(locale.get(&format!("settings.mobileRemote.guide.step{step}")), TextStyle::size(11.0).ink(Token::Ink2))
                .cell(0, 1);
            El::grid(Grid {
                children: vec![number, sentence],
                cols: vec![Track::Pixel(14.0), Track::Star(1.0)],
                col_spacing: metrics::SPACING_XS,
                ..Grid::default()
            })
        })
        .collect();
    let guide = El::stack(false, metrics::SPACING_XXS, steps).aid("settings-mobile-guide");
    let host_id_label = locale.get("settings.mobileRemote.hostIdLabel");
    let key_label = locale.get("settings.mobileRemote.keyLabel");
    let shows_key = lock(&mobile.ui).shows_key;
    let host_id = ui
        .settings_text(relay.pairing_server_id(), TextStyle::size(11.0).mono().selectable())
        .named(&host_id_label)
        .aid("settings-mobile-host-id");
    let key_text = if shows_key { relay.pairing_key_for_display().to_string() } else { "\u{2022}".repeat(16) };
    let key = ui.settings_text(key_text, TextStyle::size(11.0).mono().selectable()).named(&key_label).aid("settings-mobile-pairing-key");
    let show_label = locale.get(if shows_key { "settings.mobileRemote.hideKeyButton" } else { "settings.mobileRemote.showKeyButton" });
    let (to, toggling) = (host.clone(), mobile.clone());
    let show = ui
        .settings_push(
            ui.button(show_label.clone(), move || {
                let mut state = lock(&toggling.ui);
                state.shows_key = !state.shows_key;
                drop(state);
                crate::mobile::touch(&to);
            }),
            ControlSize::Small,
            false,
            false,
        )
        .named(show_label)
        .aid("settings-mobile-show-key");
    let (to, copy_link, note) = (host.clone(), link.clone(), copy_note.clone());
    let copy = ui
        .settings_push(
            El::button(Content::El(Box::new(ui.settings_glyph_label(
                "\u{e8c8}",
                locale.get("settings.mobileRemote.copyLinkButton"),
                11.0,
            ))))
            .on_click(move || {
                // The link carries the pairing key: out of clipboard history and cleared after a minute, as the share tab copies it.
                let (said, host) = (note.clone(), to.clone());
                crate::remote::copy_key(&to, copy_link.clone(), move |done| {
                    said.set(Some(done));
                    crate::native::touch(&host, crate::settings::SURFACE);
                });
            }),
            ControlSize::Small,
            false,
            false,
        )
        .named(locale.get("settings.mobileRemote.copyLinkButton"))
        .aid("settings-mobile-copy-link");
    let to = host.clone();
    let regenerate = ui
        .settings_push(
            El::button(Content::El(Box::new(ui.settings_glyph_label(
                "\u{e72c}",
                locale.get("settings.mobileRemote.regenerateKeyButton"),
                11.0,
            ))))
            .on_click(move || revoke(&to, None)),
            ControlSize::Small,
            false,
            true,
        )
        .named(locale.get("settings.mobileRemote.regenerateKeyButton"))
        .tooltip(locale.get("windows.mobile.rotationDescription"));
    let mut words = vec![
        guide,
        El::stack(false, metrics::SPACING_XXS, vec![caption(ui, host_id_label.clone()), host_id]),
        El::stack(false, metrics::SPACING_XXS, vec![caption(ui, key_label.clone()), El::stack(true, metrics::SPACING_SM, vec![key, show])]),
        El::stack(true, metrics::SPACING_SM, vec![copy, regenerate]),
    ];
    if let Some(done) = copy_note.get() {
        words.push(
            ui.settings_text(
                locale.get(if done { "settings.remote.pairLinkCopied" } else { "settings.remote.pairLinkCopyFailed" }),
                TextStyle::size(11.0).ink(if done { Token::Ink2 } else { Token::ErrText }),
            )
            .aid("settings-mobile-copy-note"),
        );
    }
    let words = El::stack(false, metrics::SPACING_MD, words).cell(0, 1);
    El::grid(Grid {
        children: vec![qr, words],
        cols: vec![Track::Auto, Track::Star(1.0)],
        col_spacing: metrics::SPACING_XL,
        ..Grid::default()
    })
}

/// A paired device's row: its symbol (a computer's, a phone's or the legacy apps'), its name with the "connected" and "new device"
/// marks over when it was seen, the button that unpairs it alone.
fn device_row(host: &Arc<Host>, ui: &mut Ui, device: &MobileDeviceInfo) -> El {
    let locale = ui.locale();
    let symbol = ui.settings_symbol(
        if device.desktop {
            COMPUTER_GLYPH
        } else if device.legacy {
            "\u{e897}"
        } else {
            "\u{e8ea}"
        },
        14.0,
        if device.connected { Token::DoneText } else { Token::Ink2 },
    );
    let mut name = vec![ui.settings_text(device.name.clone(), TextStyle::size(12.0))];
    if device.connected {
        let mark = ui.settings_capsule(
            locale.get("settings.mobileRemote.connectedBadge"),
            ui.brush(Token::DoneText),
            ui.brush(Token::DoneSoft),
            9.0,
            RADIUS_ROW,
        );
        name.push(mark.aid(format!("settings-mobile-connected-badge-{}", device.id)));
    }
    if device.is_new {
        let mark = ui.settings_capsule(
            locale.get("settings.mobileRemote.newDeviceBadge"),
            ui.brush(Token::WaitText),
            ui.brush(Token::WaitSoft),
            9.0,
            RADIUS_ROW,
        );
        name.push(mark.aid(format!("settings-mobile-new-badge-{}", device.id)));
    }
    let seen = locale.get_with(
        "settings.mobileRemote.seenTemplate",
        &[("first", &stamp(&locale, &device.first_seen)), ("last", &stamp(&locale, &device.last_seen))],
    );
    let words = El::stack(
        false,
        1.0,
        vec![El::stack(true, metrics::SPACING_SM, name), ui.settings_text(seen, TextStyle::size(10.0).ink(Token::Ink2))],
    )
    .va(VAlign::Center)
    .cell(0, 1);
    let (to, id) = (host.clone(), device.id.clone());
    let revoke_button = ui
        .settings_push(
            ui.button(locale.get("settings.mobileRemote.revokeRowButton"), move || revoke(&to, Some(id.clone()))),
            ControlSize::Small,
            false,
            false,
        )
        .aid(format!("settings-mobile-revoke-{}", device.id))
        .tooltip(locale.get("settings.mobileRemote.revokeOneConfirm"))
        .cell(0, 2);
    El::grid(Grid {
        children: vec![symbol, words, revoke_button],
        cols: vec![Track::Auto, Track::Star(1.0), Track::Auto],
        col_spacing: metrics::SPACING_SM,
        ..Grid::default()
    })
    .aid(format!("settings-mobile-device-{}", device.id))
}

/// The time a phone was first and last seen, in the Mac's `yyyy-MM-dd HH:mm` (local time), or the unknown-time words.
fn stamp(locale: &mighty_core::locale::Locale, value: &str) -> String {
    match mighty_core::time::parse(Some(value)) {
        Some(at) => at.to_chrono().with_timezone(&chrono::Local).format("%Y-%m-%d %H:%M").to_string(),
        None => locale.get("settings.mobileRemote.unknownTime"),
    }
}

/// C#'s `Revoke(id)`, in a danger dialog: for a row, its name and that only it is unpaired (the others stay connected; it pairs again
/// with a new code); for 키 다시 만들기, that every phone and computer is unpaired. Confirmed, the device unpaired (only while it is
/// still paired) or the key rotated.
fn revoke(host: &Arc<Host>, id: Option<String>) {
    let Some(relay) = crate::mobile::relay(host) else { return };
    if host.smoke.is_some() || id.as_ref().is_some_and(|id| !relay.devices().iter().any(|d| d.id == *id)) {
        return;
    }
    let locale = host.locale;
    let named = id.as_ref().and_then(|id| relay.devices().into_iter().find(|d| d.id == *id)).map(|d| d.name);
    let title =
        locale.get(if id.is_none() { "settings.mobileRemote.regenerateKeyButton" } else { "settings.mobileRemote.revokeDialogTitle" });
    let warning = locale.get(if id.is_none() { "windows.mobile.rotationConfirm" } else { "settings.mobileRemote.revokeOneConfirm" });
    let text = named.map(|name| format!("{name}\n\n")).unwrap_or_default() + &warning;
    let mut spec = DialogSpec::new(title, move |_| Content::Text(text.clone())).styled(None, None);
    spec.primary = Some(DialogButton::new(locale.get("settings.mobileRemote.revokeConfirmButton")));
    spec.close = Some(locale.get("settings.mobileRemote.cancelButton"));
    spec.default_button = DefaultButton::Close;
    spec.chrome = Some(DialogChrome { danger: true, ..DialogChrome::default() });
    let host = host.clone();
    crate::native::act(&host.clone(), async move {
        if crate::native::show_dialog(&host, spec).await != DialogResult::Primary {
            return Ok(());
        }
        crate::mobile::rotate(&host, id).await
    });
}

/// 모바일 원격 (C# `BuildMobileRemoteSection`, MainWindow.MobileRemote.cs:93-456) as C# draws it before the phone remote exists (W8)
/// — what its smoke draws with no host: the saved settings shown with every switch, field and button disabled (C#'s
/// `ShowControls` and its Loaded handler), the state off, and the pairing rows (QR, host id, the key as dots) and the device list
/// hidden. A press changes nothing.
fn at_rest(host: &Arc<Host>, ui: &mut Ui) -> El {
    let locale = ui.locale();
    let saved = host.view().snapshot.mobile_remote.clone().unwrap_or_default();
    let mut rows = ui.settings_rows();
    let ink3 = Some(ui.brush(Token::Ink3));
    let small = |ui: &Ui, key: &str| ui.settings_push(ui.button(locale.get(key), || {}), ControlSize::Small, false, false);
    let words = crate::platform::words();

    let allow = mighty_core::settings_words::text(&locale, words, "windows.mobile.allow").unwrap_or_default();
    let switch = ui.settings_switch(&allow, saved.enabled, Some("settings-mobile-toggle"), |_| {}).enabled(false);
    let explanation = mighty_core::settings_words::text(&locale, words, "windows.mobile.relayDescription").unwrap_or_default();
    rows.add(ui.settings_labeled(ui.settings_titled(&allow, explanation), switch, true));

    let state = El::stack(
        true,
        metrics::SPACING_SM,
        vec![
            El::new(Kind::Ellipse { fill: ink3 }).width(8.0).height(8.0).va(VAlign::Center),
            caption(ui, locale.get("settings.mobileRemote.statusOff")),
        ],
    );
    let status = El::stack(
        false,
        metrics::SPACING_XXS,
        vec![state, caption(ui, locale.get("mobileRemote.status.off")).aid("settings-mobile-detail")],
    );
    let reconnect = small(ui, "settings.mobileRemote.reconnectButton").aid("settings-mobile-reconnect").visible(false).enabled(false);
    rows.add(ui.settings_labeled(status, reconnect, true));
    // Without `--allow-state-write` the relay is never reached (a C# app on the same profile would be pushed off it): said here.
    if host.smoke.is_none() && crate::mobile::verification(host) {
        rows.add(caption(ui, crate::setup::verification_mode()).aid("settings-mobile-verification"));
    }

    // 연결 방법: the relay and the direct path's rows (mobile_direct.rs, W8-C).
    rows.add(connection(host, ui, &saved));

    // The pairing rows (title, QR, host id, key, copy and regenerate) and the devices' title: hidden without a host.
    rows.add(sub_heading(ui, locale.get("windows.mobile.pairingTitle"))).visible = false;
    let qr = El::new(Kind::Image { src: None }).width(MOBILE_QR_SIZE).height(MOBILE_QR_SIZE).aid("settings-mobile-qr");
    rows.add(qr).visible = false;
    rows.add(sub_heading(ui, locale.get("settings.mobileRemote.connectedDevicesTitle"))).visible = false;
    rows.add(advanced(host, ui, &saved, false));
    rows.finish()
}
