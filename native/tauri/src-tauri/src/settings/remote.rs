//! 원격 연결 (C# `BuildRemoteWorkspacesSection`, MainWindow.RemoteWorkspaces.cs:180-560; M/RemoteSettingsView.swift), one tab since the
//! 0.5.4 tidy-up (the owner's mockup C): the intro over three top tabs, one shown at a time.
//!
//! - 이 컴퓨터 공유: a status line with the start or stop button beside it; the workspaces to pick (or what is shared) and the
//!   full-access switch; then each way in, a row each — pairing through the relay (the switch that lets paired computers in while
//!   sharing, the relay's state, the pairing link to copy) and Tailscale (its addresses and key while sharing, the port before, or what
//!   to do when this computer has no Tailscale address).
//! - 다른 컴퓨터 연결: a pairing link or a Tailscale address with its key, the connected computers (each with how it connects, its
//!   state and its shared workspaces), and what remote workspaces can and cannot do, folded.
//! - 휴대폰: what was 모바일 리모트 (mobile.rs).
//!
//! What both the phone and computer pairing use lives once: the QR, the host id and key, 키 다시 만들기 and the relay's address are the
//! 휴대폰 tab's (a computer pastes the link, it never scans the QR); the share tab only copies the same link. The Windows Firewall rule,
//! one rule for the phone's direct port and the sharing port, is drawn once below the share and phone tabs with one set of words.
//!
//! While it is shown it reads this PC's Tailscale addresses every three seconds (remote.rs `page_tailscale`) and both sides on each
//! read. Owner: W8-D. The firewall rows are W8-C's rule (direct.rs: its state, busy mark, outcome words and presses); the rule's second
//! port is [`crate::remote::sharing_port`].

use std::sync::{Arc, Mutex};

use mighty_core::design_tokens::{Token, metrics};
use mighty_core::model::Workspace;
use mighty_native::kit::{ControlSize, TextStyle};
use mighty_native::ui::{El, Grid, HAlign, Kind, Local, Thickness, Track, Ui, VAlign, Wrapping};
use mighty_platform::FirewallRule;
use mighty_remote::direct::strings as direct_strings;
use mighty_remote::remote::wire::{RemoteConnectionInfo, RemoteConnectionStatus, RemoteHostState, protocol};
use mighty_remote::remote::{features, paired, strings};

use super::mobile_direct::MOBILE_PORT_WIDTH;
use super::{caption, folded, sub_heading};
use crate::host::{Host, lock};

/// The key as the page shows it until asked: sixteen bullets, whatever its length.
const MASKED_KEY: &str = "••••••••••••••••";

/// 원격 연결's top tabs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Share,
    Connect,
    Phone,
}

impl Tab {
    const ALL: [Tab; 3] = [Tab::Share, Tab::Connect, Tab::Phone];

    fn id(self) -> &'static str {
        match self {
            Tab::Share => "share",
            Tab::Connect => "connect",
            Tab::Phone => "phone",
        }
    }

    fn title_key(self) -> &'static str {
        match self {
            Tab::Share => "settings.remote.shareTitle",
            Tab::Connect => "settings.remote.connectTitle",
            Tab::Phone => "settings.remote.phoneTab",
        }
    }
}

/// How 다른 컴퓨터 연결 connects: a pasted pairing link through the relay, or a Tailscale address and its key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Method {
    Pairing,
    Tailscale,
}

/// The top tab shown, kept for the run rather than in the surface's locals, so a deep link can turn to one before Settings draws (the
/// sidebar's and the dashboard's connect, a 모바일 리모트 saved by 0.5.3, the help capture) and reopening Settings comes back to it.
static TAB: Mutex<Tab> = Mutex::new(Tab::Share);
/// 다른 컴퓨터 연결's way in, kept the same way.
static METHOD: Mutex<Method> = Mutex::new(Method::Pairing);

/// Turns 원격 연결 to `tab` (drawn the next time Settings draws).
pub fn show_tab(tab: Tab) {
    *TAB.lock().unwrap_or_else(|p| p.into_inner()) = tab;
}

fn tab() -> Tab {
    *TAB.lock().unwrap_or_else(|p| p.into_inner())
}

fn method() -> Method {
    *METHOD.lock().unwrap_or_else(|p| p.into_inner())
}

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

/// A small push button of the page.
fn small(ui: &Ui, title: String, enabled: bool, prominent: bool, destructive: bool, action: Box<dyn Fn() + Send + Sync>) -> El {
    ui.settings_push(ui.button(title, action).enabled(enabled), ControlSize::Small, prominent, destructive)
}

/// What every tab reads once per drawing: this PC's sharing, its Tailscale reading, the page's busy mark and error, the local
/// workspaces, the client's connections and whether paired computers are let in.
struct Page {
    tailscale: Vec<String>,
    state: RemoteHostState,
    busy: bool,
    error: Option<String>,
    locals: Vec<Shown>,
    active: Option<String>,
    opened: Vec<(String, String)>,
    sharing_host: bool,
    connections: Vec<RemoteConnectionInfo>,
    relay_on: bool,
}

fn read(host: &Arc<Host>, ui: &Ui) -> Page {
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
                // A mirror's workspace (W9-J) is not one this PC opened itself.
                let remote = w.remote.as_ref().filter(|r| !r.mirror)?;
                Some((remote.connection_id.clone()?, remote.workspace_id.clone()?))
            })
            .collect();
        (locals_of(snapshot.workspaces()), snapshot.active_workspace_id.clone(), opened)
    };
    let sharing_host = app.host().is_some();
    let connections = app.client.connections();
    // "페어링한 컴퓨터의 연결 받기": kept on the sharing host across a stop and a start; the relay link follows it while sharing.
    let relay_on = app.host().is_some_and(|sharing| sharing.relay());
    if crate::mobile::relay(host).is_some_and(|relay| relay.desktops() != (relay_on && state.enabled)) {
        let syncing = host.clone();
        tauri::async_runtime::spawn(async move { crate::remote::sync_relay(&syncing).await });
    }
    Page { tailscale, state, busy, error, locals, active, opened, sharing_host, connections, relay_on }
}

/// 원격 연결: the intro, the top tabs, the tab shown, then the error line (share and connect) and the firewall rule (share and phone).
pub fn remote_workspaces(host: &Arc<Host>, ui: &mut Ui) -> El {
    let locale = ui.locale();
    let shown = tab();
    let mut rows = ui.settings_rows();
    rows.add(caption(ui, locale.get("settings.remote.intro")));
    let options: Vec<(String, String)> = Tab::ALL.iter().map(|t| (t.id().to_string(), locale.get(t.title_key()))).collect();
    let to = host.clone();
    let tabs = ui.settings_segmented(
        locale.get("settings.remote.navTitle"),
        "settings-remote-tab",
        &options,
        shown.id(),
        move |value| {
            if let Some(picked) = Tab::ALL.into_iter().find(|t| t.id() == value) {
                show_tab(picked);
                crate::native::touch(&to, crate::settings::SURFACE);
            }
        },
        false,
        true,
    );
    rows.add(tabs);
    match shown {
        Tab::Phone => {
            rows.add(super::mobile::mobile_remote(host, ui));
            if let Some(firewall) = firewall(host, ui) {
                rows.add(firewall);
            }
        }
        Tab::Share | Tab::Connect => {
            let page = read(host, ui);
            if shown == Tab::Share {
                rows.add(share(host, ui, &page));
                if let Some(firewall) = firewall(host, ui) {
                    rows.add(firewall);
                }
            } else {
                rows.add(connect(host, ui, &page, method(), true));
            }
            rows.add(error_line(ui, &page));
        }
    }
    rows.finish()
}

/// The share and connect tabs and the firewall rule at once, for the remote smoke (smoke_remote.rs), which reads both sides on one
/// surface: the connect part on its Tailscale way in, whose button the smoke checks.
pub(crate) fn every_part(host: &Arc<Host>, ui: &mut Ui) -> El {
    let page = read(host, ui);
    let mut parts = vec![share(host, ui, &page)];
    if let Some(firewall) = firewall(host, ui) {
        parts.push(firewall);
    }
    parts.push(connect(host, ui, &page, Method::Tailscale, false));
    parts.push(error_line(ui, &page));
    El::stack(false, metrics::SPACING_MD, parts)
}

/// The last action's failure, selectable (none while there is none).
fn error_line(ui: &Ui, page: &Page) -> El {
    let mut style = TextStyle::size(11.0).ink(Token::ErrText);
    style.selectable = true;
    ui.settings_text(page.error.clone().unwrap_or_default(), style).aid("settings-remote-error").visible(page.error.is_some())
}

/// 이 컴퓨터 공유: the status line with start or stop, the workspaces (and while sharing the full-access switch), and the ways in.
fn share(host: &Arc<Host>, ui: &mut Ui, page: &Page) -> El {
    let locale = ui.locale();
    let state = &page.state;
    // What is shared, or else the workspace in front, once per showing (the page's kept values go when it is drawn anew).
    let selected = ui.local("remote-selected", || {
        if state.enabled {
            state.workspace_ids.clone()
        } else {
            page.active.iter().filter(|id| page.locals.iter().any(|l| &l.id == *id)).cloned().collect::<Vec<String>>()
        }
    });
    let shows_key = ui.local("remote-shows-key", || false);
    if !state.enabled {
        shows_key.set(false);
    }
    let port_text = ui.local("remote-port", || protocol::DEFAULT_PORT.to_string());

    // The status line: the dot, what it is doing, the remote runs, and start (not sharing) or stop (sharing) at the right.
    // No Tailscale is no fault: pairing shares without it, so the dot stays the idle one.
    let dot = if state.enabled { Token::Done } else { Token::Idle };
    let status_words = if state.enabled {
        locale.get_with("settings.remote.sharingOn", &[("count", &state.workspace_ids.len().to_string())])
    } else if page.tailscale.is_empty() && page.relay_on {
        locale.get("settings.remote.relayOnly")
    } else if page.tailscale.is_empty() {
        locale.get("settings.remote.sharingOffNoTailscale")
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
    let stopping = host.clone();
    let stop_flip = shows_key.clone();
    let stop = small(
        ui,
        locale.get("settings.remote.stopSharing"),
        !page.busy && page.sharing_host,
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
                crate::remote::sync_relay(&host).await;
                Ok(())
            });
        }),
    )
    .aid("settings-remote-stop-sharing");
    let picked = page.locals.iter().any(|l| selected.get().contains(&l.id));
    let (starting, chosen, typed) = (host.clone(), selected.clone(), port_text.clone());
    let start = small(
        ui,
        locale.get("settings.remote.startSharing"),
        !page.busy && page.sharing_host && (!page.tailscale.is_empty() || page.relay_on) && picked,
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
                let started = sharing.start(&ids, number).await.map(|_| ());
                crate::remote::sync_relay(&host).await;
                started
            });
        }),
    )
    .aid("settings-remote-start-sharing");
    let button = if state.enabled { stop } else { start };
    let status = El::grid(Grid {
        children: vec![
            El::new(Kind::Ellipse { fill: Some(ui.brush(dot)) }).width(8.0).height(8.0).va(VAlign::Center),
            caption(ui, status_words).aid("settings-remote-share-status").cell(0, 1),
            active_runs,
            button.va(VAlign::Center).cell(0, 3),
        ],
        cols: vec![Track::Auto, Track::Star(1.0), Track::Auto, Track::Auto],
        col_spacing: metrics::SPACING_SM,
        ..Grid::default()
    });
    let mut top = vec![status];
    if state.enabled {
        top.push(caption(ui, locale.get("settings.remote.stopSharingNote")));
    }

    // What is shared: the workspaces to pick, or while sharing what is shared and the full-access switch.
    let mut what = vec![];
    if state.enabled {
        what.push(sub_heading(ui, locale.get("settings.remote.sharedListTitle")));
        let shared_rows = page
            .locals
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
        what.push(El::stack(false, metrics::SPACING_XS, shared_rows));
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
        what.push(ui.settings_labeled(
            ui.settings_titled(locale.get("settings.remote.allowFullAccess"), strings::settings_allow_full_access_description()),
            full_access,
            true,
        ));
    } else {
        what.push(sub_heading(ui, locale.get("settings.remote.pickWorkspaces")));
        if page.locals.is_empty() {
            what.push(caption(ui, locale.get("settings.remote.noLocalWorkspaces")));
        }
        for local in &page.locals {
            let folder = one_line(ui.settings_text(local.path.clone(), TextStyle::size(10.0).ink(Token::Ink2).mono()));
            let content = El::stack(false, metrics::SPACING_XXS, vec![ui.settings_text(local.name.clone(), TextStyle::size(12.0)), folder]);
            let (into, me) = (selected.clone(), local.id.clone());
            what.push(
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

    let how = vec![
        sub_heading(ui, locale.get("settings.remote.methodsTitle")),
        pairing_method(host, ui, page.relay_on, state.enabled),
        tailscale_method(host, ui, page, &shows_key, &port_text),
    ];
    El::stack(
        false,
        metrics::SPACING_LG,
        vec![
            El::stack(false, metrics::SPACING_XS, top),
            El::stack(false, metrics::SPACING_SM, what),
            El::stack(false, metrics::SPACING_MD, how),
        ],
    )
}

/// 페어링: the switch that lets paired computers in through the relay while this PC shares (Tailscale then not needed), how the relay
/// link stands, and while it is on the computers' pairing link (its own short-lived code, never the phone's) to copy as a secret.
/// Without the relay host (verification mode, a smoke) the switch is off and says why.
fn pairing_method(host: &Arc<Host>, ui: &mut Ui, relay_on: bool, sharing: bool) -> El {
    let locale = ui.locale();
    let relay = crate::mobile::relay(host).filter(|_| host.smoke.is_none());
    let copy_note: Local<Option<bool>> = ui.local("remote-pair-copy-note", || None);
    let switching = host.clone();
    let accept = ui
        .settings_switch(locale.get("settings.remote.pairAccept"), relay_on, Some("settings-remote-pair-accept"), move |on| {
            if let Some(sharing) = crate::remote::app(&switching).host() {
                sharing.set_relay(on);
            }
            let host = switching.clone();
            tauri::async_runtime::spawn(async move {
                crate::remote::sync_relay(&host).await;
                crate::native::touch(&host, crate::settings::SURFACE);
            });
        })
        .enabled(relay.is_some());
    let title = El::stack(
        true,
        metrics::SPACING_SM,
        vec![
            ui.settings_text(locale.get("settings.remote.viaPairing"), TextStyle::size(12.0).medium()),
            ui.settings_text(locale.get("settings.remote.pairAccept"), TextStyle::size(12.0)),
        ],
    );
    let words = El::stack(false, metrics::SPACING_XXS, vec![title, caption(ui, locale.get("settings.remote.pairIntro"))]);
    let mut parts = vec![ui.settings_labeled(words, accept, true)];
    let status = match &relay {
        None => Some(locale.get("settings.remote.pairUnavailable")),
        Some(_) if !relay_on => None,
        Some(_) if !sharing => Some(locale.get("settings.remote.pairWaiting")),
        Some(relay) if relay.status().connected => {
            Some(locale.get_with("settings.remote.pairReady", &[("count", &relay.connected_desktops().len().to_string())]))
        }
        Some(_) => Some(locale.get("settings.remote.pairConnecting")),
    };
    if let Some(words) = status {
        parts.push(caption(ui, words).aid("settings-remote-pair-status"));
    }
    // The computers' own link (its code short-lived and spent by one pairing, which the person here allows), only while paired
    // computers are let in: with the switch off a computer's registration is refused anyway.
    if let Some(relay) = relay.as_ref().filter(|_| relay_on) {
        let (to, copied, note) = (host.clone(), relay.desktop_pairing_url(), copy_note.clone());
        // The link carries the pairing code: copied as a secret (out of clipboard history, cleared after a minute), as the key is.
        let copy = small(
            ui,
            locale.get("settings.remote.pairCopyLink"),
            true,
            false,
            false,
            Box::new(move || {
                let (said, host) = (note.clone(), to.clone());
                crate::remote::copy_key(&to, copied.clone(), move |done| {
                    said.set(Some(done));
                    crate::native::touch(&host, crate::settings::SURFACE);
                });
            }),
        )
        .aid("settings-remote-pair-copy")
        .ha(HAlign::Left);
        parts.push(copy);
        if let Some(done) = copy_note.get() {
            parts.push(
                ui.settings_text(
                    locale.get(if done { "settings.remote.pairLinkCopied" } else { "settings.remote.pairLinkCopyFailed" }),
                    TextStyle::size(11.0).ink(if done { Token::Ink2 } else { Token::ErrText }),
                )
                .aid("settings-remote-pair-copy-note"),
            );
        }
        parts.push(caption(ui, locale.get("settings.remote.pairNote")));
    }
    El::stack(false, metrics::SPACING_SM, parts).aid("settings-remote-pair")
}

/// Tailscale: while sharing its addresses and the key (dots until shown) with their copy buttons; before, the port; with no Tailscale
/// address, what to do and the way to 구성 요소, where Tailscale installs.
fn tailscale_method(host: &Arc<Host>, ui: &mut Ui, page: &Page, shows_key: &Local<bool>, port_text: &Local<String>) -> El {
    let locale = ui.locale();
    let state = &page.state;
    // What the last copy did: copied (out of history, cleared in a minute) or not; gone when the key goes.
    let copy_note: Local<Option<bool>> = ui.local("remote-copy-note", || None);
    if state.key.is_none() {
        copy_note.set(None);
    }
    let title = ui.settings_text(locale.get("settings.remote.viaTailscale"), TextStyle::size(12.0).medium());
    let mut parts =
        vec![El::stack(false, metrics::SPACING_XXS, vec![title, caption(ui, locale.get("settings.remote.tailscaleMethodNote"))])];
    let label = |ui: &Ui, key: &str| ui.settings_text(locale.get(key), TextStyle::size(12.0));
    if state.enabled {
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
            parts.push(ui.settings_labeled(
                label(ui, "settings.remote.addressLabel"),
                El::stack(true, metrics::SPACING_SM, vec![words, copy]),
                false,
            ));
        }
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
        parts.push(
            ui.settings_labeled(
                label(ui, "settings.remote.keyLabel"),
                El::stack(true, metrics::SPACING_SM, vec![key_words, show_key, copy_key]),
                false,
            )
            .visible(state.key.is_some()),
        );
        let note_words = match copy_note.get() {
            Some(true) => (strings::key_copied(), Token::Ink2),
            Some(false) => (strings::key_copy_failed(), Token::ErrText),
            None => (String::new(), Token::Ink2),
        };
        parts.push(
            ui.settings_text(note_words.0, TextStyle::size(11.0).ink(note_words.1))
                .aid("settings-remote-copy-key-note")
                .visible(copy_note.get().is_some()),
        );
    } else if page.tailscale.is_empty() {
        let to = host.clone();
        let install = small(
            ui,
            locale.get("settings.remote.tailscaleInstall"),
            true,
            false,
            false,
            Box::new(move || {
                crate::settings::select(&to, "tools");
            }),
        )
        .aid("settings-remote-tailscale-install");
        parts.push(ui.settings_labeled(caption(ui, strings::settings_tailscale_off()), install, false));
    } else {
        let typing = port_text.clone();
        let port = ui
            .settings_field(El::text_box(port_text.get()), 11.0)
            .width(MOBILE_PORT_WIDTH)
            .named(locale.get("settings.remote.portLabel"))
            .aid("settings-remote-port")
            .on_text(move |words| typing.set(words))
            .cell(0, 1);
        parts.push(El::grid(Grid {
            children: vec![label(ui, "settings.remote.portLabel"), port],
            cols: vec![Track::Auto, Track::Auto, Track::Star(1.0)],
            col_spacing: metrics::SPACING_SM,
            ..Grid::default()
        }));
        parts.push(caption(ui, strings::settings_share_note()));
    }
    El::stack(false, metrics::SPACING_SM, parts).aid("settings-remote-tailscale")
}

/// 다른 컴퓨터 연결: the way in (a pairing link or a Tailscale address; `choosable` shows the switch between them), its fields, the
/// connected computers, and what remote workspaces can and cannot do, folded.
fn connect(host: &Arc<Host>, ui: &mut Ui, page: &Page, way: Method, choosable: bool) -> El {
    let locale = ui.locale();
    let mut parts = vec![];
    if choosable {
        let options = [
            ("pairing".to_string(), locale.get("settings.remote.pairLinkField")),
            ("tailscale".to_string(), locale.get("settings.remote.tailscaleAddressTab")),
        ];
        let to = host.clone();
        parts.push(
            ui.settings_segmented(
                locale.get("settings.remote.connectTitle"),
                "settings-remote-connect-method",
                &options,
                if way == Method::Pairing { "pairing" } else { "tailscale" },
                move |value| {
                    *METHOD.lock().unwrap_or_else(|p| p.into_inner()) =
                        if value == "pairing" { Method::Pairing } else { Method::Tailscale };
                    crate::native::touch(&to, crate::settings::SURFACE);
                },
                true,
                true,
            )
            .ha(HAlign::Left)
            .width(320.0),
        );
    }
    parts.push(match way {
        Method::Pairing => pairing_client(host, ui, page.busy),
        Method::Tailscale => tailscale_client(host, ui, page.busy),
    });

    // The connected computers, each with how it connects, its state and its shared workspaces.
    if !page.connections.is_empty() {
        let mut known = vec![sub_heading(ui, locale.get("settings.remote.connectionsTitle"))];
        for connection in &page.connections {
            known.push(connection_row(host, ui, connection, &page.opened, page.busy));
        }
        parts.push(El::stack(false, metrics::SPACING_SM, known));
    }

    // What remote workspaces can and cannot do (the key line says where this OS keeps the key), folded.
    let mut scope = vec![];
    for line in [
        locale.get("settings.remote.scope.runs"),
        locale.get("settings.remote.scope.notMirrored"),
        locale.get("settings.remote.scope.outOfScope"),
        strings::settings_scope_key(),
    ] {
        scope.push(caption(ui, line));
    }
    let scope = El::stack(false, metrics::SPACING_XXS, scope).aid("settings-remote-scope");
    parts.push(folded(ui, "remote-scope-open", false, locale.get("settings.remote.scopeTitle"), scope));
    El::stack(false, metrics::SPACING_LG, parts)
}

/// The pasted pairing link (masked like a key: it carries one) and a name, connected through the relay
/// ([`mighty_remote::remote::client::RemoteClient::pair`]); the connection then lists below and in the sidebar as any other.
fn pairing_client(host: &Arc<Host>, ui: &mut Ui, busy: bool) -> El {
    let locale = ui.locale();
    let link_text = ui.local("remote-pair-link", String::new);
    let name_text = ui.local("remote-pair-name", String::new);
    let typing_link = link_text.clone();
    let link = El::new(Kind::PasswordBox { text: link_text.get(), placeholder: locale.get("settings.remote.pairLinkPlaceholder") })
        .height(metrics::SPACING_XL + 3.0)
        .va(VAlign::Center)
        .named(locale.get("settings.remote.pairLinkField"))
        .aid("settings-remote-pair-link")
        .on_text(move |words| typing_link.set(words));
    let typing_name = name_text.clone();
    let name = ui
        .settings_field(El::text_box(name_text.get()), 11.0)
        .placeholder(locale.get("settings.remote.namePlaceholder"))
        .named(locale.get("settings.remote.nameField"))
        .aid("settings-remote-pair-name")
        .on_text(move |words| typing_name.set(words));
    // A pairing waits for the person at the other computer to allow it: said under the fields meanwhile.
    let waiting = ui.local("remote-pair-waiting", || false);
    let ready = !busy && !link_text.get().trim().is_empty();
    let (pairing, name_in, link_in, asking) = (host.clone(), name_text.clone(), link_text.clone(), waiting.clone());
    let connect = small(
        ui,
        locale.get("settings.remote.connectButton"),
        ready,
        true,
        false,
        Box::new(move || {
            // The link (with its key) leaves the field before the pairing starts.
            let typed = link_in.get();
            link_in.set(String::new());
            let (name, host, asking) = (name_in.get(), pairing.clone(), asking.clone());
            asking.set(true);
            crate::remote::action(&pairing, async move {
                let paired = crate::remote::app(&host).client.pair(&name, &typed).await.map(|_| ()).map_err(|e| e.message());
                asking.set(false);
                paired
            });
        }),
    )
    .aid("settings-remote-pair-connect")
    .cell(0, 1);
    let name_line = El::grid(Grid {
        children: vec![name, connect],
        cols: vec![Track::Star(1.0), Track::Auto],
        col_spacing: metrics::SPACING_SM,
        ..Grid::default()
    });
    let label = |ui: &Ui, key: &str| ui.settings_text(locale.get(key), TextStyle::size(12.0));
    let mut parts = vec![
        ui.settings_wide(label(ui, "settings.remote.pairLinkField"), link),
        ui.settings_wide(label(ui, "settings.remote.nameField"), name_line),
        caption(ui, locale.get("settings.remote.pairConnectNote")),
    ];
    if busy && waiting.get() {
        parts.push(
            ui.settings_text(locale.get("settings.remote.pairAwaitingApproval"), TextStyle::size(11.0).ink(Token::WaitText))
                .aid("settings-remote-pair-waiting"),
        );
    }
    El::stack(false, metrics::SPACING_SM, parts).aid("settings-remote-pair-connect-part")
}

/// A Tailscale address, its key and a name, each a wide row.
fn tailscale_client(host: &Arc<Host>, ui: &mut Ui, busy: bool) -> El {
    let locale = ui.locale();
    let address_text = ui.local("remote-address", String::new);
    let key_text = ui.local("remote-key", String::new);
    let name_text = ui.local("remote-name", String::new);
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
    El::stack(
        false,
        metrics::SPACING_SM,
        vec![
            ui.settings_wide(
                label(ui, "settings.remote.addressField"),
                field(ui, "settings.remote.addressField", "settings.remote.addressPlaceholder", "settings-remote-address", &address_text),
            ),
            ui.settings_wide(label(ui, "settings.remote.keyField"), key),
            ui.settings_wide(label(ui, "settings.remote.nameField"), name_line),
            caption(ui, locale.get("settings.remote.connectNote")),
        ],
    )
    .aid("settings-remote-connect-part")
}

/// `ConnectionRow(connection, snapshot)`: its dot, name, how it connects and its state over its address; refresh (and disconnect while
/// connected) and delete; its detail; while connected, each shared workspace with the host's folder (shown only) and Open (Opened once
/// listed here).
fn connection_row(host: &Arc<Host>, ui: &mut Ui, connection: &RemoteConnectionInfo, opened: &[(String, String)], busy: bool) -> El {
    let locale = ui.locale();
    let connected = connection.status == RemoteConnectionStatus::Connected;
    let (dot, state_key) = match connection.status {
        RemoteConnectionStatus::Connected => (Token::Done, "settings.remote.status.connected"),
        RemoteConnectionStatus::Disconnected => (Token::Err, "settings.remote.status.disconnected"),
        RemoteConnectionStatus::Saved => (Token::Idle, "settings.remote.status.saved"),
    };
    let via = locale.get(if paired::is_relay_address(&connection.address) {
        "settings.remote.viaPairing"
    } else {
        "settings.remote.viaTailscale"
    });
    let name_line = El::stack(
        true,
        metrics::SPACING_SM,
        vec![
            one_line(ui.settings_text(connection.name.clone(), TextStyle::size(12.0).medium())),
            ui.settings_capsule(via, ui.brush(Token::Ink2), ui.theme().subtle(), 9.0, 8.0)
                .aid(format!("settings-remote-via-{}", connection.id)),
            ui.settings_text(locale.get(state_key), TextStyle::size(11.0).ink(Token::Ink2)),
        ],
    );
    let words = El::stack(
        false,
        metrics::SPACING_XXS,
        vec![name_line, one_line(ui.settings_text(connection.address.clone(), TextStyle::size(10.0).ink(Token::Ink2).mono()))],
    )
    .cell(0, 1);
    let id = connection.id.clone();
    let mut actions = vec![];
    let (to, which) = (host.clone(), id.clone());
    actions.push(
        small(
            ui,
            locale.get("settings.remote.refreshButton"),
            !busy,
            false,
            false,
            Box::new(move || {
                let (host, which) = (to.clone(), which.clone());
                crate::remote::action(&to, async move {
                    crate::remote::app(&host).client.refresh(&which).await.map(|_| ()).map_err(|e| e.message())
                });
            }),
        )
        .aid(format!("settings-remote-refresh-{id}")),
    );
    if connected {
        let (to, which) = (host.clone(), id.clone());
        actions.push(
            small(
                ui,
                locale.get("settings.remote.disconnectButton"),
                !busy,
                false,
                false,
                Box::new(move || {
                    let leaving = crate::remote::app(&to).client.disconnect(&which);
                    crate::remote::action(&to, async move {
                        leaving.await;
                        Ok(())
                    });
                }),
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
        row.push(caption(ui, detail.clone()));
    }
    if connected {
        if connection.shared().is_empty() {
            row.push(caption(ui, locale.get("settings.remote.noShared")));
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
            // 보기 shows the host's own panes (the remote mirror, W9-J); a host without the mirror (it answered 404) keeps 열기.
            let mirrored = crate::mirror::supported(&connection.id);
            let label = if mirrored {
                locale.get("remote.mirror.view")
            } else {
                locale.get(if listed { "settings.remote.openedButton" } else { "settings.remote.openButton" })
            };
            let open = small(
                ui,
                label,
                !busy,
                false,
                false,
                Box::new(move || {
                    let (host, connection_id, workspace_id) = (to.clone(), connection_id.clone(), workspace_id.clone());
                    crate::remote::action(&to, async move {
                        if mirrored && crate::mirror::show_workspace(&host, &connection_id, &workspace_id).await {
                            return Ok(());
                        }
                        crate::remote::open_workspace(&host, &connection_id, &workspace_id)
                    });
                }),
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

/// The Windows Firewall rule (W8-C's, one rule for the phone's direct port and the sharing port): its state, one note, add (or update
/// in place for the ports in use now, or read again while it could not be read) and delete; one UAC prompt per change, none in a smoke
/// run. Read (unelevated) each time the page is shown, unless a change still waits on its prompt (C#'s Loaded). None on the Mac,
/// which has no firewall step (`PlatformProfile::direct_connection_firewall`, M-C).
pub(crate) fn firewall(host: &Arc<Host>, ui: &mut Ui) -> Option<El> {
    if !crate::direct::has_firewall(host) {
        return None;
    }
    let locale = ui.locale();
    let smoke = host.smoke.is_some();
    let read = ui.local("remote-firewall-read", || false);
    if !read.get() {
        read.set(true);
        crate::direct::settings_opened(host);
    }
    let firewall = crate::direct::firewall_view(host);
    let firewall_ports = crate::direct::firewall_ports(host);
    let unknown = firewall.rule == FirewallRule::Unreadable;
    let present = matches!(firewall.rule, FirewallRule::Present { .. });
    let state_words = format!(
        "{}{}{}",
        direct_strings::firewall_state(&locale, &firewall.rule),
        crate::direct::firewall_needs_update_words(host),
        firewall.message.as_ref().map(|message| format!(" · {message}")).unwrap_or_default()
    );
    let words = El::stack(
        false,
        metrics::SPACING_XXS,
        vec![caption(ui, state_words).aid("settings-remote-firewall-state"), caption(ui, strings::settings_firewall_note())],
    );
    let add_label = if unknown {
        direct_strings::firewall_recheck_button(&locale)
    } else if present {
        strings::firewall_update()
    } else {
        strings::firewall_add()
    };
    // Adding, removing and even reading the rule run netsh: never in a smoke, and only with --allow-state-write (D4).
    let free = !smoke && !firewall.busy && firewall.may_change;
    let to = host.clone();
    let add = small(ui, add_label.clone(), free, false, false, Box::new(move || crate::direct::firewall_add_pressed(&to)))
        .named(&add_label)
        .aid("settings-remote-firewall-add")
        // With no port to open and no rule to read again there is nothing to press, so the button is not shown at all.
        .visible(unknown || !firewall_ports.is_empty());
    let to = host.clone();
    let remove = small(ui, strings::firewall_remove(), free, false, true, Box::new(move || crate::direct::firewall_remove_pressed(&to)))
        .aid("settings-remote-firewall-remove")
        .visible(present);
    let buttons = El::stack(true, metrics::SPACING_SM, vec![add, remove]);
    Some(El::stack(
        false,
        metrics::SPACING_SM,
        vec![sub_heading(ui, locale.get("settings.remote.firewallTitle")), ui.settings_labeled(words, buttons, true)],
    ))
}
