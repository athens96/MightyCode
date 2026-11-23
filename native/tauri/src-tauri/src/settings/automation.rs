//! 자동화 (MC-9), beside the notifications in Mighty Pet과 알림: the states announced, the webhooks (masked, each with its test and
//! removal, then an address field), the status hook's command, and the command line's state with 토큰 새로 만들기. Built from the
//! control module's memory only (the addresses were read from the keychain as the app started); every change runs on a task.

use std::sync::Arc;

use mighty_core::control::events::StateEvent;
use mighty_core::design_tokens::metrics;
use mighty_native::kit::{ControlSize, TextStyle};
use mighty_native::ui::{El, Kind, Ui};

use super::{caption, sub_heading};
use crate::control::{self, webhooks};
use crate::host::Host;

/// The fields' width.
const FIELD_WIDTH: f64 = 300.0;

fn redraw(host: &Host) {
    crate::native::touch(host, crate::settings::SURFACE);
}

pub fn automation(host: &Arc<Host>, ui: &mut Ui) -> El {
    let locale = ui.locale();
    let mut rows = ui.settings_rows();
    let small = |ui: &Ui, title: String, press: Box<dyn Fn() + Send + Sync>| {
        ui.settings_push(ui.button(title, press), ControlSize::Small, false, false)
    };
    rows.add(caption(ui, locale.get("automation.settings.description")));

    // The states hooks and webhooks announce.
    rows.add(sub_heading(ui, locale.get("automation.settings.events")));
    let chosen = control::chosen_events(host);
    for event in StateEvent::ALL {
        let label = webhooks::event_word(&locale, event);
        let to = host.clone();
        let toggle =
            ui.settings_switch(&label, chosen.contains(&event), Some(&format!("settings-automation-event-{}", event.name())), move |on| {
                control::set_event(&to, event, on);
                redraw(&to);
            });
        rows.add(ui.settings_labeled(ui.settings_text(&label, TextStyle::default()), toggle, false));
    }

    // The webhooks.
    rows.add(sub_heading(ui, locale.get("automation.webhooks.title")));
    let urls = webhooks::urls();
    if urls.is_empty() {
        rows.add(caption(ui, locale.get("automation.webhooks.empty")));
    }
    for (index, url) in urls.iter().enumerate() {
        let shown = format!("{} · {}", webhooks::kind_of(url).label(), webhooks::masked(url));
        let to = host.clone();
        let test = small(
            ui,
            locale.get("automation.webhooks.test"),
            Box::new(move || {
                let host = to.clone();
                crate::native::act(&to, async move {
                    webhooks::test(&host, index).await;
                    Ok(())
                });
            }),
        )
        .aid(format!("settings-automation-webhook-test-{index}"));
        let to = host.clone();
        let remove = small(
            ui,
            locale.get("automation.webhooks.remove"),
            Box::new(move || {
                let host = to.clone();
                crate::native::act(&to, async move {
                    let removed = webhooks::remove(&host, index).await;
                    webhooks::report(&host, removed);
                    Ok(())
                });
            }),
        )
        .aid(format!("settings-automation-webhook-remove-{index}"));
        let buttons = El::new(Kind::Panel { children: vec![test, remove] });
        rows.add(ui.settings_labeled(ui.settings_text(shown, TextStyle::size(12.0).selectable()), buttons, false));
    }
    let typed = ui.local("automation-webhook-url", String::new);
    let into = typed.clone();
    let field = ui
        .settings_field(El::text_box(typed.get()).placeholder("https://hooks.slack.com/…").on_text(move |text| into.set(text)), 11.0)
        .width(FIELD_WIDTH)
        .named(locale.get("automation.webhooks.title"))
        .aid("settings-automation-webhook-url");
    let (to, adding) = (host.clone(), typed.clone());
    let add = small(
        ui,
        locale.get("automation.webhooks.add"),
        Box::new(move || {
            let (host, text) = (to.clone(), adding.get());
            let clear = adding.clone();
            crate::native::act(&to, async move {
                let added = webhooks::add(&host, &text).await;
                if added.is_ok() {
                    clear.set(String::new());
                }
                webhooks::report(&host, added);
                Ok(())
            });
        }),
    )
    .aid("settings-automation-webhook-add")
    .enabled(!typed.get().trim().is_empty() && urls.len() < webhooks::MAX_WEBHOOKS);
    rows.add(El::stack(true, metrics::SPACING_SM, vec![field, add]));
    rows.add(caption(ui, locale.get("automation.webhooks.note")));
    if let Some(message) = webhooks::message() {
        rows.add(caption(ui, message).aid("settings-automation-webhook-message"));
    }

    // The status hook.
    rows.add(sub_heading(ui, locale.get("automation.hook.title")));
    let saved = control::hook_command(host);
    let command = ui.local("automation-hook-command", || saved.clone());
    let into = command.clone();
    let field = ui
        .settings_field(El::text_box(command.get()).placeholder("say \"$MIGHTY_STATE\"").on_text(move |text| into.set(text)), 11.0)
        .width(FIELD_WIDTH)
        .named(locale.get("automation.hook.title"))
        .aid("settings-automation-hook");
    let (to, typed_command) = (host.clone(), command.clone());
    let save = small(
        ui,
        locale.get("automation.hook.save"),
        Box::new(move || {
            control::set_hook_command(&to, &typed_command.get());
            redraw(&to);
        }),
    )
    .aid("settings-automation-hook-save")
    .enabled(command.get().trim() != saved.trim());
    rows.add(El::stack(true, metrics::SPACING_SM, vec![field, save]));
    rows.add(caption(ui, locale.get("automation.hook.note")));

    // The command line.
    rows.add(sub_heading(ui, locale.get("automation.api.title")));
    let listening = control::listening();
    let to = host.clone();
    let rotate = small(
        ui,
        locale.get("automation.api.rotate"),
        Box::new(move || {
            let said = match control::rotate_token(&to) {
                Ok(()) => to.locale.get("automation.api.rotated"),
                Err(error) => to.locale.get_with("automation.webhooks.failed", &[("reason", &error)]),
            };
            *API_MESSAGE.lock().unwrap_or_else(|p| p.into_inner()) = Some(said);
            redraw(&to);
        }),
    )
    .aid("settings-automation-api-rotate")
    .enabled(listening);
    let state = locale.get(if listening { "automation.api.listening" } else { "automation.api.off" });
    rows.add(ui.settings_labeled(ui.settings_text(state, TextStyle::size(12.0)), rotate, false));
    rows.add(caption(ui, locale.get("automation.api.note")));
    let api_message = API_MESSAGE.lock().unwrap_or_else(|p| p.into_inner()).clone();
    if let Some(message) = api_message {
        rows.add(caption(ui, message).aid("settings-automation-api-message"));
    }
    rows.finish()
}

/// What the last 토큰 새로 만들기 said.
static API_MESSAGE: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);
