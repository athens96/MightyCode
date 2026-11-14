//! 원격 작업 공간 (C# `BuildRemoteWorkspacesSection`, MainWindow.RemoteWorkspaces.cs:180-560), moved as it was from `later.rs` in W8
//! Step 0. Owner: W8-D.

use std::sync::Arc;

use mighty_core::design_tokens::{Token, metrics};
use mighty_native::kit::{ControlSize, TextStyle};
use mighty_native::ui::{El, Grid, Kind, Track, Ui, VAlign, Wrapping};

use super::mobile_direct::MOBILE_PORT_WIDTH;
use crate::host::Host;

/// `RemoteProtocol.DefaultPort`.
const REMOTE_PORT: &str = "43137";

/// 원격 작업 공간 (C# `BuildRemoteWorkspacesSection`, MainWindow.RemoteWorkspaces.cs:180-560) as C# draws it with no sharing host and no
/// remote client (W8): not sharing, the local workspaces to pick (the active one picked), the port, start and connect disabled, the
/// firewall rule unknown (re-check, disabled in a smoke), and the scope lines.
pub fn remote_workspaces(host: &Arc<Host>, ui: &mut Ui) -> El {
    let locale = ui.locale();
    let (locals, active) = {
        let view = host.view();
        let locals: Vec<(String, String, String)> = view
            .snapshot
            .workspaces()
            .iter()
            .filter(|w| w.remote.is_none())
            .map(|w| (w.id().to_string(), w.name().to_string(), w.path().to_string()))
            .collect();
        (locals, view.snapshot.active_workspace_id.clone())
    };
    let caption = |ui: &Ui, key: &str| ui.settings_text(locale.get(key), TextStyle::size(11.0).ink(Token::Ink2));
    let title = |ui: &Ui, key: &str| ui.settings_text(locale.get(key), TextStyle::default().medium());
    let mut rows = ui.settings_rows();
    rows.add(caption(ui, "settings.remote.intro"));

    let status = El::grid(Grid {
        children: vec![
            El::new(Kind::Ellipse { fill: Some(ui.brush(Token::Idle)) }).width(8.0).height(8.0).va(VAlign::Center),
            caption(ui, "settings.remote.sharingOff").aid("settings-remote-share-status").cell(0, 1),
        ],
        cols: vec![Track::Auto, Track::Star(1.0), Track::Auto],
        col_spacing: metrics::SPACING_SM,
        ..Grid::default()
    });
    let selected = ui
        .local("remote-selected", || active.iter().filter(|id| locals.iter().any(|(l, _, _)| l == *id)).cloned().collect::<Vec<String>>());
    let mut picks = vec![];
    if locals.is_empty() {
        picks.push(caption(ui, "settings.remote.noLocalWorkspaces"));
    } else {
        picks.push(ui.settings_text(locale.get("settings.remote.pickWorkspaces"), TextStyle::size(11.0).ink(Token::Ink2).medium()));
        for (id, name, path) in &locals {
            let mut folder = ui.settings_text(path.clone(), TextStyle::size(10.0).ink(Token::Ink2).mono());
            if let Some(text) = folder.text_mut() {
                text.wrapping = Wrapping::NoWrap;
            }
            let content = El::stack(false, metrics::SPACING_XXS, vec![ui.settings_text(name.clone(), TextStyle::size(12.0)), folder]);
            let (into, me) = (selected.clone(), id.clone());
            picks.push(
                El::check_box(selected.get().contains(id), content)
                    .named(name.clone())
                    .aid(format!("settings-remote-share-{id}"))
                    .keyed(id.clone())
                    .on_toggle(move |on| {
                        into.update(|list| {
                            list.retain(|x| *x != me);
                            if on {
                                list.push(me.clone());
                            }
                        })
                    }),
            );
        }
    }
    let port = ui
        .settings_field(El::text_box(REMOTE_PORT), 11.0)
        .width(MOBILE_PORT_WIDTH)
        .named(locale.get("settings.remote.portLabel"))
        .aid("settings-remote-port")
        .cell(0, 1);
    let start = ui
        .settings_push(ui.button(locale.get("settings.remote.startSharing"), || {}).enabled(false), ControlSize::Small, true, false)
        .aid("settings-remote-start-sharing")
        .cell(0, 3);
    let port_line = El::grid(Grid {
        children: vec![ui.settings_text(locale.get("settings.remote.portLabel"), TextStyle::size(12.0)), port, start],
        cols: vec![Track::Auto, Track::Auto, Track::Star(1.0), Track::Auto],
        col_spacing: metrics::SPACING_SM,
        ..Grid::default()
    });
    let not_sharing = El::stack(
        false,
        metrics::SPACING_SM,
        vec![El::stack(false, metrics::SPACING_XS, picks), port_line, caption(ui, "remote.windows.settings.shareNote")],
    );
    rows.add(El::stack(false, metrics::SPACING_SM, vec![title(ui, "settings.remote.shareTitle"), status, not_sharing]));

    let words = El::stack(
        false,
        metrics::SPACING_XXS,
        vec![
            caption(ui, "mobileRemote.direct.windows.firewall.unknown").aid("settings-remote-firewall-state"),
            caption(ui, "remote.windows.settings.firewallNote"),
            caption(ui, "remote.windows.firewall.removeClosesBoth"),
        ],
    );
    // The firewall's check is W7's (remote workspaces): until then the button does nothing and is shown disabled.
    let buttons = El::stack(
        true,
        metrics::SPACING_SM,
        vec![
            ui.settings_push(ui.button(locale.get("setup.button.recheck"), || {}).enabled(false), ControlSize::Small, false, false)
                .aid("settings-remote-firewall-add"),
            ui.settings_push(ui.button(locale.get("remote.windows.firewall.remove"), || {}), ControlSize::Small, false, true)
                .aid("settings-remote-firewall-remove")
                .visible(false),
        ],
    );
    rows.add(ui.settings_labeled(words, buttons, true));

    let field = |ui: &Ui, key: &str, placeholder: &str, aid: &str, size: f64| {
        ui.settings_field(El::text_box(""), size).placeholder(locale.get(placeholder)).named(locale.get(key)).aid(aid)
    };
    let key = El::new(Kind::PasswordBox { text: String::new(), placeholder: locale.get("settings.remote.keyPlaceholder") })
        .height(metrics::SPACING_XL + 3.0)
        .va(VAlign::Center)
        .named(locale.get("settings.remote.keyField"))
        .aid("settings-remote-key");
    let connect = ui
        .settings_push(ui.button(locale.get("settings.remote.connectButton"), || {}).enabled(false), ControlSize::Small, true, false)
        .aid("settings-remote-connect")
        .cell(0, 1);
    let name_line = El::grid(Grid {
        children: vec![field(ui, "settings.remote.nameField", "settings.remote.namePlaceholder", "settings-remote-name", 12.0), connect],
        cols: vec![Track::Star(1.0), Track::Auto],
        col_spacing: metrics::SPACING_SM,
        ..Grid::default()
    });
    let label = |ui: &Ui, key: &str| ui.settings_text(locale.get(key), TextStyle::size(12.0));
    rows.add(El::stack(
        false,
        metrics::SPACING_SM,
        vec![
            title(ui, "settings.remote.connectTitle"),
            ui.settings_wide(
                label(ui, "settings.remote.addressField"),
                field(ui, "settings.remote.addressField", "settings.remote.addressPlaceholder", "settings-remote-address", 11.0),
            ),
            ui.settings_wide(label(ui, "settings.remote.keyField"), key),
            ui.settings_wide(label(ui, "settings.remote.nameField"), name_line),
            caption(ui, "settings.remote.connectNote"),
        ],
    ));
    let mut scope = vec![title(ui, "settings.remote.scopeTitle")];
    for key in [
        "settings.remote.scope.runs",
        "settings.remote.scope.notMirrored",
        "settings.remote.scope.outOfScope",
        "remote.windows.settings.scopeKey",
    ] {
        scope.push(caption(ui, key));
    }
    rows.add(El::stack(false, metrics::SPACING_XXS, scope).aid("settings-remote-scope"));
    rows.finish()
}
