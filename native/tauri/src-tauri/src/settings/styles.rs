//! The styles box (C# `BuildStylesSection`, MainWindow.Styles.cs:18-139; M/StyleSettingsSection.swift:16-108): the lock banner while
//! the trust file cannot be read, the register and rescan buttons, the explanation, a row per style — its name beside the state
//! capsule, its figures, its path, its small buttons — and a faded row per file that was refused. Owner: W9-D.
//!
//! The registry is read off the dispatcher (styles.rs) and kept here until the next rescan or decision; the box is drawn from it.
//! Register, allow, revoke, unblock and remove write only where `Host::writes` allows (their buttons are off otherwise).

use std::sync::{Arc, Mutex};

use mighty_core::design_tokens::{Token, metrics};
use mighty_native::kit::{ControlSize, TextStyle};
use mighty_native::ui::{El, Ui};
use mighty_styles::{RegisteredStyle, StyleRegistry, presentation};

use crate::host::Host;

/// The registry the box shows, and whether a read is out.
static SHOWN: Mutex<(Option<Arc<StyleRegistry>>, bool)> = Mutex::new((None, false));

/// Reads the registry again and draws the box with it (C# `Populate`).
pub fn rescan(host: &Arc<Host>) {
    {
        let mut shown = SHOWN.lock().unwrap_or_else(|p| p.into_inner());
        if shown.1 {
            return;
        }
        shown.1 = true;
    }
    let host = host.clone();
    tauri::async_runtime::spawn(async move {
        let registry = crate::styles::settings_registry(&host).await;
        *SHOWN.lock().unwrap_or_else(|p| p.into_inner()) = (Some(Arc::new(registry)), false);
        crate::native::touch(&host, crate::settings::SURFACE);
    });
}

/// The picked manifest file (C#'s `FileOpenPicker` with `.json`).
async fn pick(host: &Host) -> Option<std::path::PathBuf> {
    use tauri::Manager;
    use tauri_plugin_dialog::DialogExt;
    let (sender, receiver) = tokio::sync::oneshot::channel();
    let mut dialog = host.app.dialog().file().add_filter("JSON", &["json"]);
    if let Some(window) = host.app.get_window(crate::host::WINDOW) {
        dialog = dialog.set_parent(&window);
    }
    dialog.pick_file(move |file| {
        let _ = sender.send(file);
    });
    receiver.await.ok().flatten().and_then(|file| file.into_path().ok())
}

/// A button whose work runs on its own task and draws the box again after it.
fn work<F, Fut>(host: &Arc<Host>, run: F) -> impl Fn() + Send + Sync + 'static
where
    F: Fn(Arc<Host>) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = Result<(), String>> + Send + 'static,
{
    let host = host.clone();
    move || {
        let host = host.clone();
        let task = run(host.clone());
        crate::native::act(&host.clone(), async move {
            let result = task.await;
            SHOWN.lock().unwrap_or_else(|p| p.into_inner()).0 = None;
            rescan(&host);
            result
        });
    }
}

/// 스타일 (C# `BuildStylesSection`).
pub fn styles(host: &Arc<Host>, ui: &mut Ui) -> El {
    let locale = ui.locale();
    let writes = host.writes();
    let registry = SHOWN.lock().unwrap_or_else(|p| p.into_inner()).0.clone();
    if registry.is_none() {
        rescan(host);
    }
    let locked = registry.as_ref().is_some_and(|r| r.trust_locked);
    let mut rows = ui.settings_rows();
    if locked {
        // The file that is locked, in 11pt medium wait text, over what to do about it (M/StyleSettingsSection.swift:42-51).
        let file = host.data_dir.join("style-trust").join("approvals.json").display().to_string();
        rows.add(
            El::stack(
                false,
                metrics::SPACING_XXS,
                vec![
                    ui.settings_text(file, TextStyle::size(11.0).ink(Token::WaitText).medium().selectable()),
                    ui.settings_text(locale.get("settings.styles.lockBanner"), TextStyle::size(11.0).ink(Token::Ink2).selectable()),
                ],
            )
            .aid("settings-style-locked"),
        );
    }
    let register = work(host, |host| async move {
        let Some(file) = pick(&host).await else { return Ok(()) };
        crate::styles::register_file(&host, file).await
    });
    let register = ui
        .settings_push(
            ui.button(locale.get("settings.styles.registerButton"), register).enabled(writes && !locked),
            ControlSize::Regular,
            false,
            false,
        )
        .aid("settings-style-register");
    let again = host.clone();
    let rescan_button = ui
        .settings_push(
            ui.button(locale.get("settings.styles.rescanButton"), move || {
                SHOWN.lock().unwrap_or_else(|p| p.into_inner()).0 = None;
                rescan(&again);
                crate::styles::reload_all(&again);
            }),
            ControlSize::Regular,
            false,
            false,
        )
        .aid("settings-style-rescan");
    rows.add(El::stack(true, metrics::SPACING_SM, vec![register, rescan_button]));
    rows.add(ui.settings_text(locale.get("settings.styles.description"), TextStyle::size(11.0).ink(Token::Ink2)));
    for style in registry.iter().flat_map(|r| r.styles.iter()) {
        rows.add(style_row(host, ui, style, locked, writes));
    }
    for rejection in registry.iter().flat_map(|r| r.rejections.iter()) {
        // Why the file was refused over its path, at 0.7 (M/StyleSettingsSection.swift:99-108).
        rows.add(
            El::stack(
                false,
                metrics::SPACING_XXS,
                vec![
                    ui.settings_text(
                        format!("{} · {}", rejection.code, rejection.message),
                        TextStyle::size(11.0).ink(Token::Ink2).selectable(),
                    ),
                    ui.settings_tertiary(rejection.path.clone(), 10.0, true, true),
                ],
            )
            .opacity(0.7)
            .aid("settings-style-rejected"),
        );
    }
    rows.finish().aid("settings-style-rows")
}

/// One style's row: its name beside the state capsule, its id, hash, action and tool counts, its path, then view, the decision and
/// (a user style) remove.
fn style_row(host: &Arc<Host>, ui: &mut Ui, style: &RegisteredStyle, locked: bool, writes: bool) -> El {
    let locale = ui.locale();
    let count = |key: &str, n: usize| locale.get_with(key, &[("count", &n.to_string())]);
    // A style the app ships reads the bundled badge's one word here, as the Mac's list does (M/Styles/StyleSurfaces.swift:248).
    let capsule = if style.approval == "preApproved" {
        locale.get("settings.toolkit.bundledBadge")
    } else {
        presentation::state(&locale, &style.approval)
    };
    let head = El::stack(
        true,
        metrics::SPACING_SM,
        vec![
            ui.settings_text(presentation::name(&locale, style), TextStyle::size(12.0).medium()),
            ui.settings_capsule(capsule, ui.brush(Token::Ink2), ui.theme().subtle(), 10.0, 8.0),
        ],
    );
    let figures = format!(
        "{} · {} · {} · {}",
        style.id(),
        locale.get_with("settings.styles.hashDetailTemplate", &[("hash", &style.hash[..12.min(style.hash.len())])]),
        count("settings.styles.actionCountTemplate", style.manifest.actions.len()),
        count("settings.styles.autoAllowCountTemplate", style.manifest.auto_allow().len())
    );
    let mut controls = vec![];
    let viewed = style.clone();
    let view = work(host, move |host| {
        let style = viewed.clone();
        async move {
            crate::styles::show_approval(&host, &style, true).await;
            Ok(())
        }
    });
    controls.push(ui.settings_push(ui.button(locale.get("settings.styles.viewButton"), view), ControlSize::Small, false, false));
    if style.source != "bundled" {
        let action = if style.approval == "revoked" {
            "unblock"
        } else if style.runnable() {
            "revoked"
        } else {
            "approved"
        };
        let key = match action {
            "unblock" => "settings.styles.unblockButton",
            "revoked" => "settings.styles.revokeButton",
            _ => "settings.styles.allowButton",
        };
        let decided = style.clone();
        let decide = work(host, move |host| {
            let style = decided.clone();
            async move { crate::styles::decide(&host, style, action).await }
        });
        controls.push(ui.settings_push(ui.button(locale.get(key), decide).enabled(writes && !locked), ControlSize::Small, false, false));
    }
    if style.source == "user" {
        let removed = style.clone();
        let remove = work(host, move |host| {
            let style = removed.clone();
            async move { crate::styles::remove(&host, style).await }
        });
        controls.push(ui.settings_push(
            ui.button(locale.get("settings.styles.removeButton"), remove).enabled(writes && !locked),
            ControlSize::Small,
            false,
            true,
        ));
    }
    El::stack(
        false,
        metrics::SPACING_XS,
        vec![
            head,
            ui.settings_text(figures, TextStyle::size(10.0).ink(Token::Ink2)),
            ui.settings_tertiary(style.path.clone(), 10.0, true, true),
            El::stack(true, metrics::SPACING_SM, controls),
        ],
    )
    .aid(format!("settings-style-{}", style.id()))
}
