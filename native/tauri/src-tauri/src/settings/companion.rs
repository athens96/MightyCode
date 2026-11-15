//! 컴패니언 (C# `BuildCompanionSection`, MainWindow.Companion.cs:510-578). Owner: W8-E.
//!
//! The pet switch; the chosen pet's first frame (58×64) beside the pet pop-up over the import, reload and reset buttons; the 10pt note;
//! the bubble, motion and completion-notification switches; the notification state beside the settings button; then the last message.
//! The switches are `companion-settings.json`'s (companion.rs keeps them; written only with `--allow-state-write`), the completion
//! switch writes both that file's `notifications` and the app's setting, as C# does. Where the pet may not show (without
//! `--allow-state-write`, and in a smoke) the last row says verification mode; an import, which writes a pet into the profile, is off
//! there too.

#![deny(clippy::significant_drop_in_scrutinee)]

use std::path::PathBuf;
use std::sync::Arc;

use base64::Engine;
use mighty_companion::pet::CompanionPet;
use mighty_core::design_tokens::{Token, metrics};
use mighty_native::kit::{ControlSize, TextStyle};
use mighty_native::ui::{El, Grid, HAlign, Kind, Thickness, Track, Ui, VAlign};
use tauri::Manager;

use crate::companion::{self, CompanionApp};
use crate::host::Host;

fn lock<T>(mutex: &std::sync::Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// The preview's PNG of a pet, as a data URL, made once per pet (C# `CompanionPreview`: the atlas's first frame, 192 × 208).
static PREVIEW: std::sync::Mutex<Option<(String, Option<String>)>> = std::sync::Mutex::new(None);

fn preview(host: &Arc<Host>, selected: Option<&CompanionPet>) -> Option<String> {
    let selected = selected?;
    let kept = lock(&PREVIEW).clone();
    if let Some((id, url)) = kept
        && id == selected.id
    {
        return url;
    }
    *lock(&PREVIEW) = Some((selected.id.clone(), None));
    let (source, id) = (selected.source.clone(), selected.id.clone());
    let host = host.clone();
    tauri::async_runtime::spawn(async move {
        let made = tauri::async_runtime::spawn_blocking(move || {
            CompanionPet::load(&source, &id).and_then(|pet| pet.decode()).and_then(|atlas| mighty_companion::pet::preview_png(&atlas))
        })
        .await
        .unwrap_or_else(|e| Err(e.to_string()));
        let mut kept = lock(&PREVIEW);
        if let Some((_, url)) = kept.as_mut() {
            match made {
                Ok(png) => *url = Some(format!("data:image/png;base64,{}", base64::engine::general_purpose::STANDARD.encode(png))),
                Err(error) => {
                    if let Some(companion) = companion::app(&host) {
                        lock_state(&companion).error = Some(error);
                    }
                }
            }
        }
        drop(kept);
        crate::native::touch(&host, crate::settings::SURFACE);
    });
    None
}

fn lock_state(companion: &CompanionApp) -> std::sync::MutexGuard<'_, companion::State> {
    companion.state()
}

/// A pet file the user picks (C#'s `FileOpenPicker` with `.json`, `.png`, `.webp`), owned by the app's window.
async fn pick_pet(host: &Host) -> Option<PathBuf> {
    use tauri_plugin_dialog::DialogExt;
    let (sender, receiver) = tokio::sync::oneshot::channel();
    let mut dialog = host.app.dialog().file().add_filter("Pet", &["json", "png", "webp"]);
    if let Some(window) = host.app.get_window(crate::host::WINDOW) {
        dialog = dialog.set_parent(&window);
    }
    dialog.pick_file(move |file| {
        let _ = sender.send(file);
    });
    receiver.await.ok().flatten().and_then(|file| file.into_path().ok())
}

/// Changes the settings, saves them, and reads the catalog and the window again (C#'s switch and button handlers).
fn change(host: &Arc<Host>, edit: impl FnOnce(&mut mighty_companion::prefs::CompanionPreferences) + Send + 'static, reload: bool) {
    let host = host.clone();
    crate::native::act(&host.clone(), async move {
        if let Some(companion) = companion::app(&host) {
            edit(&mut lock_state(&companion).prefs);
            companion.save(&host);
            if reload {
                companion::reload(&host).await;
            } else {
                companion::refresh(&host);
            }
        }
        crate::native::touch(&host, crate::settings::SURFACE);
        Ok(())
    });
}

/// 컴패니언 (C# `BuildCompanionSection`).
pub fn companion(host: &Arc<Host>, ui: &mut Ui) -> El {
    let locale = ui.locale();
    let mut rows = ui.settings_rows();
    let Some(app) = companion::app(host) else { return rows.finish() };
    let (preferences, pets, loaded, error) = {
        let state = lock_state(&app);
        (state.prefs.clone(), state.pets.clone(), state.loaded.clone(), state.error.clone())
    };
    let switch = |ui: &mut Ui,
                  rows: &mut mighty_native::kit::SettingsRows,
                  label: String,
                  aid: &str,
                  on: bool,
                  toggled: Box<dyn Fn(bool) + Send + Sync>| {
        let toggle = ui.settings_switch(&label, on, Some(aid), toggled);
        rows.add(ui.settings_labeled(ui.settings_text(&label, TextStyle::default()), toggle, false));
    };
    let to = host.clone();
    switch(
        ui,
        &mut rows,
        locale.get("companion.settings.enabled"),
        "settings-companion-enabled",
        preferences.enabled,
        Box::new(move |on| {
            // Turned off, the pet goes and the status bar's paw says so at once; the catalog is read again behind them.
            change(&to, move |p| p.enabled = on, true);
        }),
    );
    let pet_label = locale.get("companion.settings.pet");
    let shown_id = loaded.clone().unwrap_or_else(|| preferences.selected_pet.clone());
    let items =
        pets.iter().map(|pet| format!("{}{}", pet.name, if pet.id.starts_with("codex:") { " · Codex" } else { "" }).into()).collect();
    let index = pets.iter().position(|pet| pet.id == shown_id).map_or(-1, |i| i as i32);
    let ids: Vec<String> = pets.iter().map(|pet| pet.id.clone()).collect();
    let to = host.clone();
    let picker = ui
        .settings_popup(
            El::combo_box(index, items).on_select(move |chosen| {
                let Some(id) = usize::try_from(chosen).ok().and_then(|i| ids.get(i)).cloned() else { return };
                change(&to, move |p| p.selected_pet = id, true);
            }),
            false,
        )
        .ha(HAlign::Right)
        .named(pet_label.clone())
        .aid("settings-companion-pet");
    let framed = ui.settings_popup_frame(picker).margin(Thickness::new(0.0, 0.0, -mighty_native::kit::SETTINGS_POPUP_CHIP_INSET, 0.0));
    let writes = host.writes();
    let to = host.clone();
    let import = ui.settings_push(
        ui.button(locale.get("companion.settings.import"), move || {
            let host = to.clone();
            crate::native::act(&host.clone(), async move {
                let Some(path) = pick_pet(&host).await else { return Ok(()) };
                let profile = host.data_dir.clone();
                // The GUI fully decodes and checks alpha before installing; the validated bytes are what is written.
                let installed = tauri::async_runtime::spawn_blocking(move || {
                    let pet = CompanionPet::load(&path, "import")?;
                    pet.decode()?;
                    pet.install(&profile)
                })
                .await
                .unwrap_or_else(|e| Err(e.to_string()));
                if let Some(companion) = companion::app(&host) {
                    match installed {
                        Ok(pet) => {
                            lock_state(&companion).prefs.selected_pet = pet.id;
                            companion.save(&host);
                            companion::reload(&host).await;
                        }
                        Err(error) => lock_state(&companion).error = Some(error),
                    }
                }
                crate::native::touch(&host, crate::settings::SURFACE);
                Ok(())
            });
        })
        .enabled(writes),
        ControlSize::Small,
        false,
        false,
    );
    let to = host.clone();
    let reload = ui.settings_push(
        ui.button(locale.get("companion.settings.reload"), move || {
            let host = to.clone();
            crate::native::act(&host.clone(), async move {
                *lock(&PREVIEW) = None;
                companion::reload(&host).await;
                crate::native::touch(&host, crate::settings::SURFACE);
                Ok(())
            });
        }),
        ControlSize::Small,
        false,
        false,
    );
    let to = host.clone();
    let reset = ui.settings_push(
        ui.button(locale.get("companion.settings.resetPosition"), move || {
            change(
                &to,
                |p| {
                    p.left = None;
                    p.top = None;
                    p.bubble_width = None;
                    p.bubble_height = None;
                },
                true,
            );
        }),
        ControlSize::Small,
        false,
        false,
    );
    let choice = El::stack(
        false,
        metrics::SPACING_SM,
        vec![
            ui.settings_labeled(ui.settings_text(&pet_label, TextStyle::default()), framed, false),
            El::new(Kind::Panel { children: vec![import, reload, reset] }),
        ],
    )
    .va(VAlign::Center)
    .cell(0, 1);
    let selected = pets.iter().find(|p| p.id == preferences.selected_pet).or_else(|| pets.first());
    let source = preview(host, selected);
    let preview = El::new(Kind::Image { src: source }).width(58.0).height(64.0).va(VAlign::Center).aid("settings-companion-preview");
    rows.add(El::grid(Grid {
        children: vec![preview, choice],
        cols: vec![Track::Auto, Track::Star(1.0)],
        col_spacing: metrics::SPACING_SM,
        ..Grid::default()
    }));
    rows.add(ui.settings_text(locale.get("companion.settings.description"), TextStyle::size(10.0).ink(Token::Ink2)));
    let to = host.clone();
    switch(
        ui,
        &mut rows,
        locale.get("companion.settings.task"),
        "settings-companion-task",
        preferences.shows_task,
        Box::new(move |on| {
            let host = to.clone();
            if let Some(companion) = companion::app(&host) {
                companion.set_shows_task(on);
            }
            change(&host, move |p| p.shows_task = on, false);
        }),
    );
    let to = host.clone();
    switch(
        ui,
        &mut rows,
        locale.get("companion.settings.motion"),
        "settings-companion-motion",
        preferences.reduced_motion,
        Box::new(move |on| change(&to, move |p| p.reduced_motion = on, false)),
    );
    // One switch for the one setting: the section's own label; it writes the companion's file and the app's setting.
    let label = locale.get("windows.notifications.toggleLabel");
    let on = host.view().snapshot.completion_notifications_enabled;
    let to = host.clone();
    let toggle = ui.settings_switch(&label, on, Some("settings-completion-notifications"), move |on| {
        change(&to, move |p| p.notifications = on, false);
        let host = to.clone();
        crate::native::act(&to, async move { host.view().update(|state| state.completion_notifications_enabled = on) });
    });
    rows.add(ui.settings_labeled(ui.settings_text(&label, TextStyle::default()), toggle, false));
    let words = crate::notifications::permission_text(&locale, crate::notifications::permission(host));
    let to = host.clone();
    let open = ui.settings_push(
        ui.button(locale.get("windows.notifications.settingsButton"), move || {
            if to.smoke.is_none() {
                let _ = crate::platform::open_url("ms-settings:notifications");
            }
        }),
        ControlSize::Small,
        false,
        false,
    );
    rows.add(ui.settings_labeled(ui.settings_text(words, TextStyle::size(11.0).ink(Token::Ink2)), open, false));
    // The last message: what went wrong, else (where the pet may not show) verification mode.
    let message = error.or_else(|| (!companion::may_show(host)).then(|| locale.get("windows.notifications.statusVerificationMode")));
    if let Some(message) = message.filter(|m| !m.is_empty()) {
        rows.add(ui.settings_text(message, TextStyle::size(11.0).ink(Token::Ink2)).aid("settings-companion-message"));
    }
    rows.finish()
}
