//! 컴패니언 (C# `BuildCompanionSection`, MainWindow.Companion.cs:510-578), moved as it was from `later.rs` in W8 Step 0. Owner: W8-E
//! (the switches saved to `companion-settings.json` instead of `PET_SWITCHES`, the picker, the preview, import, reload, reset position;
//! the completion switch writes both).

use std::sync::{Arc, Mutex};

use mighty_core::design_tokens::{Token, metrics};
use mighty_native::kit::{ControlSize, TextStyle};
use mighty_native::ui::{El, Grid, HAlign, Kind, Thickness, Track, Ui, VAlign};

use crate::host::Host;

/// The companion's pet switches as switched in this run, by automation id (W8 saves them).
static PET_SWITCHES: Mutex<Vec<(String, bool)>> = Mutex::new(Vec::new());

fn pet_switches() -> std::sync::MutexGuard<'static, Vec<(String, bool)>> {
    PET_SWITCHES.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// 컴패니언 (C# `BuildCompanionSection`, MainWindow.Companion.cs:510-578). The pets (their catalog, picker, preview, import, reload and
/// position) are W8's: until then no pet is installed, so the picker is empty and the box says so (`companion.error.noPet`); the pet
/// switches keep what was switched for this run (across categories; their defaults at first), and the pet's buttons are shown disabled.
/// The completion notification switch is the saved setting, and the row under it says
/// whether Windows lets the app notify (notifications.rs).
pub fn companion(host: &Arc<Host>, ui: &mut Ui) -> El {
    let locale = ui.locale();
    let mut rows = ui.settings_rows();
    let switch = |ui: &mut Ui, rows: &mut mighty_native::kit::SettingsRows, key: &str, aid: &str, on: bool| {
        let label = locale.get(key);
        let kept = pet_switches().iter().find(|(id, _)| id == aid).map_or(on, |(_, on)| *on);
        let id = aid.to_string();
        let toggle = ui.settings_switch(&label, kept, Some(aid), move |on| {
            let mut switches = pet_switches();
            switches.retain(|(kept, _)| *kept != id);
            switches.push((id.clone(), on));
        });
        rows.add(ui.settings_labeled(ui.settings_text(&label, TextStyle::default()), toggle, false));
    };
    switch(ui, &mut rows, "companion.settings.enabled", "settings-companion-enabled", true);
    let picker = ui
        .settings_popup(El::combo_box(-1, vec![]), false)
        .ha(HAlign::Right)
        .named(locale.get("companion.settings.pet"))
        .aid("settings-companion-pet");
    let framed = ui.settings_popup_frame(picker).margin(Thickness::new(0.0, 0.0, -mighty_native::kit::SETTINGS_POPUP_CHIP_INSET, 0.0));
    let buttons = ["companion.settings.import", "companion.settings.reload", "companion.settings.resetPosition"]
        .iter()
        .map(|key| ui.settings_push(ui.button(locale.get(key), || {}).enabled(false), ControlSize::Small, false, false))
        .collect();
    let choice = El::stack(
        false,
        metrics::SPACING_SM,
        vec![
            ui.settings_labeled(ui.settings_text(locale.get("companion.settings.pet"), TextStyle::default()), framed, false),
            El::new(Kind::Panel { children: buttons }),
        ],
    )
    .va(VAlign::Center)
    .cell(0, 1);
    let preview = El::new(Kind::Image { src: None }).width(58.0).height(64.0).va(VAlign::Center).aid("settings-companion-preview");
    rows.add(El::grid(Grid {
        children: vec![preview, choice],
        cols: vec![Track::Auto, Track::Star(1.0)],
        col_spacing: metrics::SPACING_SM,
        ..Grid::default()
    }));
    rows.add(ui.settings_text(locale.get("companion.settings.description"), TextStyle::size(10.0).ink(Token::Ink2)));
    switch(ui, &mut rows, "companion.settings.task", "settings-companion-task", true);
    switch(ui, &mut rows, "companion.settings.motion", "settings-companion-motion", false);

    let label = locale.get("windows.notifications.toggleLabel");
    let on = host.view().snapshot.completion_notifications_enabled;
    let to = host.clone();
    let toggle = ui.settings_switch(&label, on, Some("settings-completion-notifications"), move |on| {
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
    rows.add(ui.settings_text(locale.get("companion.error.noPet"), TextStyle::size(11.0).ink(Token::Ink2)));
    rows.finish()
}
