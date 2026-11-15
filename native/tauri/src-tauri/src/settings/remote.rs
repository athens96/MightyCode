//! 원격 작업 공간 (C# `BuildRemoteWorkspacesSection`, MainWindow.RemoteWorkspaces.cs:180-560; M/RemoteSettingsView.swift): the intro;
//! sharing this PC (its addresses, the key, the full-access switch, what is shared, or the workspaces to pick and the port); the Windows
//! Firewall rule; connecting to another computer; the connected computers with their shared workspaces; and what remote workspaces can
//! and cannot do. While it is shown it reads this PC's Tailscale addresses every three seconds (remote.rs `page_tailscale`) and both
//! sides on each read. Owner: W8-D.
//!
//! The firewall rows are W8-C's rule (shared with the phone's direct path, direct.rs): until it reports its state here they read
//! unknown with their buttons off, as Step 0 drew them; the rule's second port is [`crate::remote::sharing_port`].

use std::sync::Arc;

use mighty_core::design_tokens::{Token, metrics};
use mighty_core::model::Workspace;
use mighty_native::kit::{ControlSize, TextStyle};
use mighty_native::ui::{El, Grid, Kind, Local, Thickness, Track, Ui, VAlign, Wrapping};
use mighty_remote::remote::features;
use mighty_remote::remote::strings;
use mighty_remote::remote::wire::{RemoteConnectionInfo, RemoteConnectionStatus, protocol};

use super::mobile_direct::MOBILE_PORT_WIDTH;
use crate::host::{Host, lock};

/// The key as the page shows it until asked: sixteen bullets, whatever its length.
const MASKED_KEY: &str = "••••••••••••••••";

/// A local workspace as the page lists it: its id, name and folder (`LocalPath`).
pub(crate) struct Shown {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) path: String,
}

pub(crate) fn locals_of(workspaces: &[Workspace]) -> Vec<Shown> {
    workspaces
        .iter()
        .filter_map(|w| {
            let path = features::local_path(Some(w))?;
            Some(Shown { id: w.id().to_string(), name: w.name().to_string(), path: path.to_string() })
        })
        .collect()
}

/// Words with no wrapping, cut with an ellipsis (C#'s `NoWrap` + `CharacterEllipsis`).
fn one_line(mut words: El) -> El {
    if let Some(text) = words.text_mut() {
        text.wrapping = Wrapping::NoWrap;
    }
    words
}

/// 원격 작업 공간 as C# draws it, from the host's state, the client's connections and this PC's Tailscale reading.
pub fn remote_workspaces(host: &Arc<Host>, ui: &mut Ui) -> El {
    let locale = ui.locale();
    let app = crate::remote::app(host);
    let tailscale = app.page_tailscale(host);
    // `Current()` ends a sharing whose listener died before it answers, so the page never shows that sharing as on.
    let state = crate::remote::host_state(host);
    let (busy, error) = {
        let mut page = lock(&app.page);
        if page.sharing_shown && !state.enabled && !page.busy {
            page.error = Some(locale.get("remote.host.error.stopped"));
        }
        page.sharing_shown = state.enabled;
        (page.busy, page.error.clone())
    };
    let (locals, active, opened) = {
        let view = host.view();
        let snapshot = &view.snapshot;
        let opened: Vec<(String, String)> = snapshot
            .workspaces()
            .iter()
            .filter_map(|w| {
                let remote = w.remote.as_ref()?;
                Some((remote.connection_id.clone()?, remote.workspace_id.clone()?))
            })
            .collect();
        (locals_of(snapshot.workspaces()), snapshot.active_workspace_id.clone(), opened)
    };
    let sharing_host = app.host().is_some();
    let connections = app.client.connections();

    // What is shared, or else the workspace in front, once per showing (the page's kept values go when it is drawn anew).
    let selected = ui.local("remote-selected", || {
        if state.enabled {
            state.workspace_ids.clone()
        } else {
            active.iter().filter(|id| locals.iter().any(|l| &l.id == *id)).cloned().collect::<Vec<String>>()
        }
    });
    let shows_key = ui.local("remote-shows-key", || false);
    if !state.enabled {
        shows_key.set(false);
    }
    // What the last copy did: copied (out of history, cleared in a minute) or not; gone when the key goes.
    let copy_note: Local<Option<bool>> = ui.local("remote-copy-note", || None);
    if state.key.is_none() {
        copy_note.set(None);
    }
    let port_text = ui.local("remote-port", || protocol::DEFAULT_PORT.to_string());
    let address_text = ui.local("remote-address", String::new);
    let key_text = ui.local("remote-key", String::new);
    let name_text = ui.local("remote-name", String::new);

    let caption = |ui: &Ui, text: String| ui.settings_text(text, TextStyle::size(11.0).ink(Token::Ink2));
    let heading = |ui: &Ui, key: &str| ui.settings_text(locale.get(key), TextStyle::size(13.0).medium());
    let small = |ui: &Ui, title: String, enabled: bool, prominent: bool, destructive: bool, action: Box<dyn Fn() + Send + Sync>| {
        ui.settings_push(ui.button(title, action).enabled(enabled), ControlSize::Small, prominent, destructive)
    };
    let mut rows = ui.settings_rows();
    rows.add(caption(ui, locale.get("settings.remote.intro")));

    // Share this computer: the heading and its status line.
    let dot = if state.enabled {
        Token::Done
    } else if tailscale.is_empty() {
        Token::Err
    } else {
        Token::Idle
    };
    let status_words = if state.enabled {
        locale.get_with("settings.remote.sharingOn", &[("count", &state.workspace_ids.len().to_string())])
    } else if tailscale.is_empty() {
        strings::settings_tailscale_off()
    } else {
        locale.get("settings.remote.sharingOff")
    };
    let active_runs = ui
        .settings_text(
            locale.get_with("settings.remote.activeRuns", &[("count", &state.active_runs.to_string())]),
            TextStyle::size(11.0).ink(Token::WaitText),
        )
        .aid("settings-remote-active-runs")
        .visible(state.enabled && state.active_runs > 0)
        .cell(0, 2);
    let status = El::grid(Grid {
        children: vec![
            El::new(Kind::Ellipse { fill: Some(ui.brush(dot)) }).width(8.0).height(8.0).va(VAlign::Center),
            caption(ui, status_words).aid("settings-remote-share-status").cell(0, 1),
            active_runs,
        ],
        cols: vec![Track::Auto, Track::Star(1.0), Track::Auto],
        col_spacing: metrics::SPACING_SM,
        ..Grid::default()
    });

    // While sharing: the addresses, the key, the full-access switch, what is shared, and the stop button.
    let mut sharing = vec![];
    let mut addresses = vec![];
    for value in &state.addresses {
        let words = one_line(ui.settings_text(value.clone(), TextStyle::size(11.0).mono().selectable()));
        let (to, copied) = (host.clone(), value.clone());
        let copy = small(
            ui,
            locale.get("settings.remote.copyButton"),
            true,
            false,
            false,
            Box::new(move || crate::remote::copy_text(&to, copied.clone())),
        )
        .aid("settings-remote-copy-address");
        addresses.push(ui.settings_labeled(
            ui.settings_text(locale.get("settings.remote.addressLabel"), TextStyle::size(12.0)),
            El::stack(true, metrics::SPACING_SM, vec![words, copy]),
            false,
        ));
    }
    sharing.push(El::stack(false, metrics::SPACING_XS, addresses));
    let shown_key = match (&state.key, shows_key.get()) {
        (Some(key), true) => key.clone(),
        _ => MASKED_KEY.to_string(),
    };
    let mut key_style = TextStyle::size(11.0).mono();
    key_style.selectable = shows_key.get();
    let key_words =
        one_line(ui.settings_text(shown_key, key_style)).named(locale.get("settings.remote.keyLabel")).aid("settings-remote-key-text");
    let show_label = locale.get(if shows_key.get() { "settings.remote.hideKeyButton" } else { "settings.remote.showKeyButton" });
    let flip = shows_key.clone();
    let show_key =
        small(ui, show_label, true, false, false, Box::new(move || flip.update(|on| *on = !*on))).aid("settings-remote-show-key");
    let (to, key, note) = (host.clone(), state.key.clone(), copy_note.clone());
    let copy_key = small(
        ui,
        locale.get("settings.remote.copyButton"),
        true,
        false,
        false,
        Box::new(move || {
            if let Some(value) = key.clone() {
                let (said, host) = (note.clone(), to.clone());
                crate::remote::copy_key(&to, value, move |done| {
                    said.set(Some(done));
                    crate::native::touch(&host, crate::settings::SURFACE);
                });
            }
        }),
    )
    .aid("settings-remote-copy-key")
    .tooltip(locale.get("settings.remote.copyButton"));
    let key_row = ui
        .settings_labeled(
            ui.settings_text(locale.get("settings.remote.keyLabel"), TextStyle::size(12.0)),
            El::stack(true, metrics::SPACING_SM, vec![key_words, show_key, copy_key]),
            false,
        )
        .visible(state.key.is_some());
    sharing.push(key_row);
    let note_words = match copy_note.get() {
        Some(true) => (strings::key_copied(), Token::Ink2),
        Some(false) => (strings::key_copy_failed(), Token::ErrText),
        None => (String::new(), Token::Ink2),
    };
    sharing.push(
        ui.settings_text(note_words.0, TextStyle::size(11.0).ink(note_words.1))
            .aid("settings-remote-copy-key-note")
            .visible(copy_note.get().is_some()),
    );
    // "Allow full access and commands from remote": for the sharing now going, off whenever sharing starts.
    let switching = host.clone();
    let full_access = ui.settings_switch(
        locale.get("settings.remote.allowFullAccess"),
        state.allows_full_access,
        Some("settings-remote-full-access"),
        move |on| {
            if let Some(sharing) = crate::remote::app(&switching).host()
                && sharing.state().enabled
            {
                sharing.set_allows_full_access(on);
            }
        },
    );
    sharing.push(ui.settings_labeled(
        ui.settings_titled(locale.get("settings.remote.allowFullAccess"), strings::settings_allow_full_access_description()),
        full_access,
        true,
    ));
    sharing.push(ui.settings_text(locale.get("settings.remote.sharedListTitle"), TextStyle::size(11.0).ink(Token::Ink2).medium()));
    let shared_rows = locals
        .iter()
        .filter(|l| state.workspace_ids.contains(&l.id))
        .map(|l| {
            El::stack(
                true,
                metrics::SPACING_SM,
                vec![ui.settings_symbol("\u{E8B7}", 12.0, Token::Ink2), ui.settings_text(l.name.clone(), TextStyle::size(12.0))],
            )
            .tooltip(l.path.clone())
            .keyed(l.id.clone())
        })
        .collect();
    sharing.push(El::stack(false, metrics::SPACING_XS, shared_rows));
    let stopping = host.clone();
    let stop_flip = shows_key.clone();
    let stop = small(
        ui,
        locale.get("settings.remote.stopSharing"),
        !busy && sharing_host,
        false,
        true,
        Box::new(move || {
            stop_flip.set(false);
            let host = stopping.clone();
            crate::remote::action(&stopping, async move {
                let app = crate::remote::app(&host);
                lock(&app.page).sharing_shown = false;
                if let Some(sharing) = app.host().cloned() {
                    sharing.stop().await;
                }
                Ok(())
            });
        }),
    )
    .ha(mighty_native::ui::HAlign::Right)
    .aid("settings-remote-stop-sharing");
    sharing.push(stop);
    sharing.push(caption(ui, locale.get("settings.remote.stopSharingNote")));

    // While not sharing: the workspaces to pick, the port and the start button.
    let mut picks = vec![];
    if locals.is_empty() {
        picks.push(caption(ui, locale.get("settings.remote.noLocalWorkspaces")));
    } else {
        picks.push(ui.settings_text(locale.get("settings.remote.pickWorkspaces"), TextStyle::size(11.0).ink(Token::Ink2).medium()));
        for local in &locals {
            let folder = one_line(ui.settings_text(local.path.clone(), TextStyle::size(10.0).ink(Token::Ink2).mono()));
            let content = El::stack(false, metrics::SPACING_XXS, vec![ui.settings_text(local.name.clone(), TextStyle::size(12.0)), folder]);
            let (into, me) = (selected.clone(), local.id.clone());
            picks.push(
                El::check_box(selected.get().contains(&local.id), content)
                    .named(local.name.clone())
                    .aid(format!("settings-remote-share-{}", local.id))
                    .keyed(local.id.clone())
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
    let typing = port_text.clone();
    let port = ui
        .settings_field(El::text_box(port_text.get()), 11.0)
        .width(MOBILE_PORT_WIDTH)
        .named(locale.get("settings.remote.portLabel"))
        .aid("settings-remote-port")
        .on_text(move |words| typing.set(words))
        .cell(0, 1);
    let picked = locals.iter().any(|l| selected.get().contains(&l.id));
    let (starting, chosen, typed) = (host.clone(), selected.clone(), port_text.clone());
    let start = small(
        ui,
        locale.get("settings.remote.startSharing"),
        !busy && sharing_host && !tailscale.is_empty() && picked,
        true,
        false,
        Box::new(move || {
            let host = starting.clone();
            let ids: Vec<String> = {
                let view = host.view();
                let chosen = chosen.get();
                view.snapshot
                    .workspaces()
                    .iter()
                    .filter(|w| w.remote.is_none())
                    .map(|w| w.id().to_string())
                    .filter(|id| chosen.contains(id))
                    .collect()
            };
            // As on the Mac, a port that is not a number starts on the default one; the host refuses one out of range (a number past
            // 65535 is handed on as 1, which the host refuses with the same words, after the same earlier checks).
            let words = typed.get();
            let words = words.trim();
            let number = if !words.is_empty() && words.chars().all(|c| c.is_ascii_digit()) {
                words.parse::<u32>().map_or(1, |n| u16::try_from(n).unwrap_or(1))
            } else {
                protocol::DEFAULT_PORT
            };
            crate::remote::action(&starting, async move {
                // Listening is a machine change: only with `--allow-state-write`, never in a smoke (W8 plan D-rules).
                if !host.may_listen() {
                    return Err(mighty_run::strings::get("windows.notifications.statusVerificationMode"));
                }
                let Some(sharing) = crate::remote::app(&host).host().cloned() else { return Ok(()) };
                sharing.start(&ids, number).await.map(|_| ())
            });
        }),
    )
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
        vec![El::stack(false, metrics::SPACING_XS, picks), port_line, caption(ui, strings::settings_share_note())],
    )
    .visible(!state.enabled);
    let sharing = El::stack(false, metrics::SPACING_SM, sharing).visible(state.enabled);
    rows.add(El::stack(false, metrics::SPACING_SM, vec![heading(ui, "settings.remote.shareTitle"), status, sharing, not_sharing]));

    // The Windows Firewall rule (W8-C's, shared with the phone's direct path): unknown with its buttons off until direct.rs reports it.
    let words = El::stack(
        false,
        metrics::SPACING_XXS,
        vec![
            caption(ui, locale.get("mobileRemote.direct.windows.firewall.unknown")).aid("settings-remote-firewall-state"),
            caption(ui, strings::settings_firewall_note()),
            caption(ui, strings::firewall_remove_closes_both()),
        ],
    );
    let buttons = El::stack(
        true,
        metrics::SPACING_SM,
        vec![
            small(ui, locale.get("setup.button.recheck"), false, false, false, Box::new(|| {})).aid("settings-remote-firewall-add"),
            small(ui, strings::firewall_remove(), false, false, true, Box::new(|| {}))
                .aid("settings-remote-firewall-remove")
                .visible(false),
        ],
    );
    rows.add(ui.settings_labeled(words, buttons, true));

    // Connect to another computer: the address, the key and a name, each a wide row.
    let field = |ui: &Ui, key: &str, placeholder: &str, aid: &str, text: &Local<String>| {
        let into = text.clone();
        ui.settings_field(El::text_box(text.get()), 11.0)
            .placeholder(locale.get(placeholder))
            .named(locale.get(key))
            .aid(aid)
            .on_text(move |words| into.set(words))
    };
    let typing_key = key_text.clone();
    let key = El::new(Kind::PasswordBox { text: key_text.get(), placeholder: locale.get("settings.remote.keyPlaceholder") })
        .height(metrics::SPACING_XL + 3.0)
        .va(VAlign::Center)
        .named(locale.get("settings.remote.keyField"))
        .aid("settings-remote-key")
        .on_text(move |words| typing_key.set(words));
    let ready = !busy && !address_text.get().trim().is_empty() && !key_text.get().trim().is_empty();
    let (connecting, name_in, address_in, key_in) = (host.clone(), name_text.clone(), address_text.clone(), key_text.clone());
    let connect = small(
        ui,
        locale.get("settings.remote.connectButton"),
        ready,
        true,
        false,
        Box::new(move || {
            // The key leaves the field before the connect starts.
            let typed = key_in.get();
            key_in.set(String::new());
            let (name, address, host) = (name_in.get(), address_in.get(), connecting.clone());
            crate::remote::action(&connecting, async move {
                crate::remote::app(&host).client.connect(&name, &address, &typed).await.map(|_| ()).map_err(|e| e.message())
            });
        }),
    )
    .aid("settings-remote-connect")
    .cell(0, 1);
    let name_line = El::grid(Grid {
        children: vec![
            field(ui, "settings.remote.nameField", "settings.remote.namePlaceholder", "settings-remote-name", &name_text),
            connect,
        ],
        cols: vec![Track::Star(1.0), Track::Auto],
        col_spacing: metrics::SPACING_SM,
        ..Grid::default()
    });
    let label = |ui: &Ui, key: &str| ui.settings_text(locale.get(key), TextStyle::size(12.0));
    rows.add(El::stack(
        false,
        metrics::SPACING_SM,
        vec![
            heading(ui, "settings.remote.connectTitle"),
            ui.settings_wide(
                label(ui, "settings.remote.addressField"),
                field(ui, "settings.remote.addressField", "settings.remote.addressPlaceholder", "settings-remote-address", &address_text),
            ),
            ui.settings_wide(label(ui, "settings.remote.keyField"), key),
            ui.settings_wide(label(ui, "settings.remote.nameField"), name_line),
            caption(ui, locale.get("settings.remote.connectNote")),
        ],
    ));

    let mut error_style = TextStyle::size(11.0).ink(Token::ErrText);
    error_style.selectable = true;
    rows.add(ui.settings_text(error.clone().unwrap_or_default(), error_style).aid("settings-remote-error")).visible = error.is_some();

    // The connected computers, each with its shared workspaces.
    let mut known = vec![];
    if !connections.is_empty() {
        known.push(heading(ui, "settings.remote.connectionsTitle"));
    }
    for connection in &connections {
        known.push(connection_row(host, ui, connection, &opened, busy));
    }
    rows.add(El::stack(false, metrics::SPACING_SM, known)).visible = !connections.is_empty();

    // What remote workspaces can and cannot do (the key line says where Windows keeps the key).
    let mut scope = vec![heading(ui, "settings.remote.scopeTitle")];
    for line in [
        locale.get("settings.remote.scope.runs"),
        locale.get("settings.remote.scope.notMirrored"),
        locale.get("settings.remote.scope.outOfScope"),
        strings::settings_scope_key(),
    ] {
        scope.push(caption(ui, line));
    }
    rows.add(El::stack(false, metrics::SPACING_XXS, scope).aid("settings-remote-scope"));
    rows.finish()
}

/// `ConnectionRow(connection, snapshot)`: its dot, name and address, refresh (and disconnect while connected) and remove; its detail;
/// while connected, each shared workspace with the host's folder (shown only) and Open (Opened once listed here).
fn connection_row(host: &Arc<Host>, ui: &mut Ui, connection: &RemoteConnectionInfo, opened: &[(String, String)], busy: bool) -> El {
    let locale = ui.locale();
    let connected = connection.status == RemoteConnectionStatus::Connected;
    let dot = match connection.status {
        RemoteConnectionStatus::Connected => Token::Done,
        RemoteConnectionStatus::Disconnected => Token::Err,
        RemoteConnectionStatus::Saved => Token::Idle,
    };
    let words = El::stack(
        false,
        metrics::SPACING_XXS,
        vec![
            one_line(ui.settings_text(connection.name.clone(), TextStyle::size(12.0).medium())),
            one_line(ui.settings_text(connection.address.clone(), TextStyle::size(10.0).ink(Token::Ink2).mono())),
        ],
    )
    .cell(0, 1);
    let id = connection.id.clone();
    let mut actions = vec![];
    let (to, which) = (host.clone(), id.clone());
    actions.push(
        ui.settings_push(
            ui.button(locale.get("settings.remote.refreshButton"), move || {
                let (host, which) = (to.clone(), which.clone());
                crate::remote::action(&to, async move {
                    crate::remote::app(&host).client.refresh(&which).await.map(|_| ()).map_err(|e| e.message())
                });
            })
            .enabled(!busy),
            ControlSize::Small,
            false,
            false,
        )
        .aid(format!("settings-remote-refresh-{id}")),
    );
    if connected {
        let (to, which) = (host.clone(), id.clone());
        actions.push(
            ui.settings_push(
                ui.button(locale.get("settings.remote.disconnectButton"), move || {
                    let leaving = crate::remote::app(&to).client.disconnect(&which);
                    crate::remote::action(&to, async move {
                        leaving.await;
                        Ok(())
                    });
                })
                .enabled(!busy),
                ControlSize::Small,
                false,
                false,
            )
            .aid(format!("settings-remote-disconnect-{id}")),
        );
    }
    let (to, which) = (host.clone(), id.clone());
    actions.push(
        ui.settings_icon_button(
            "\u{E74D}",
            locale.get("settings.remote.removeHelp"),
            move || {
                let leaving = crate::remote::app(&to).client.remove(&which);
                crate::remote::action(&to, async move {
                    leaving.await;
                    Ok(())
                });
            },
            12.0,
        )
        .enabled(!busy)
        .aid(format!("settings-remote-remove-{id}")),
    );
    let head = El::grid(Grid {
        children: vec![
            El::new(Kind::Ellipse { fill: Some(ui.brush(dot)) }).width(8.0).height(8.0).va(VAlign::Center),
            words,
            El::stack(true, metrics::SPACING_SM, actions).va(VAlign::Center).cell(0, 2),
        ],
        cols: vec![Track::Auto, Track::Star(1.0), Track::Auto],
        col_spacing: metrics::SPACING_SM,
        ..Grid::default()
    });
    let mut row = vec![head];
    if let Some(detail) = &connection.detail {
        row.push(ui.settings_text(detail.clone(), TextStyle::size(11.0).ink(Token::Ink2)));
    }
    if connected {
        if connection.shared().is_empty() {
            row.push(ui.settings_text(locale.get("settings.remote.noShared"), TextStyle::size(11.0).ink(Token::Ink2)));
        }
        for hosted in connection.shared() {
            let listed = opened.iter().any(|(c, w)| *c == connection.id && *w == hosted.id);
            let labels = El::stack(
                false,
                metrics::SPACING_XXS,
                vec![
                    ui.settings_text(hosted.name.clone(), TextStyle::size(12.0)),
                    // The host's folder, shown only: it is that computer's path, never one on this PC.
                    one_line(ui.settings_text(hosted.path.clone(), TextStyle::size(10.0).ink(Token::Ink2).mono())),
                ],
            )
            .cell(0, 1);
            let (to, connection_id, workspace_id) = (host.clone(), connection.id.clone(), hosted.id.clone());
            let open = ui
                .settings_push(
                    ui.button(locale.get(if listed { "settings.remote.openedButton" } else { "settings.remote.openButton" }), move || {
                        let (host, connection_id, workspace_id) = (to.clone(), connection_id.clone(), workspace_id.clone());
                        crate::remote::action(&to, async move { crate::remote::open_workspace(&host, &connection_id, &workspace_id) });
                    })
                    .enabled(!busy),
                    ControlSize::Small,
                    false,
                    false,
                )
                .aid(format!("settings-remote-open-{}", hosted.id))
                .cell(0, 2);
            row.push(
                El::grid(Grid {
                    children: vec![ui.settings_symbol("\u{E977}", 12.0, Token::Ink2), labels, open],
                    cols: vec![Track::Auto, Track::Star(1.0), Track::Auto],
                    col_spacing: metrics::SPACING_SM,
                    ..Grid::default()
                })
                .margin(Thickness::new(metrics::SPACING_XL, 0.0, 0.0, 0.0))
                .keyed(hosted.id.clone()),
            );
        }
    }
    El::stack(false, metrics::SPACING_XS, row).aid(format!("settings-remote-connection-{id}")).keyed(id)
}
