//! SSH 머신 (MC-33), in 원격 연결 after the remote box: each machine with its state, 연결 테스트 and 지우기, the tmux switch (on
//! where the test found tmux), a remote folder field with 열기; then a `Host` picked from `~/.ssh/config` or `user@host[:port]` typed.
//! Built from the ssh module's memory only (the config's aliases are read on a task); every change runs on a task.

use std::sync::Arc;

use mighty_core::design_tokens::metrics;
use mighty_native::kit::{ControlSize, TextStyle};
use mighty_native::ui::{El, Kind, MenuItem, Ui};

use super::{caption, sub_heading};
use crate::host::Host;
use crate::ssh;

/// The fields' width.
const FIELD_WIDTH: f64 = 260.0;

pub fn ssh_machines(host: &Arc<Host>, ui: &mut Ui) -> El {
    let locale = ui.locale();
    let mut rows = ui.settings_rows();
    let small = |ui: &Ui, title: String, press: Box<dyn Fn() + Send + Sync>| {
        ui.settings_push(ui.button(title, press), ControlSize::Small, false, false)
    };
    rows.add(caption(ui, locale.get("ssh.settings.description")));
    if ssh::ssh_program().is_none() {
        rows.add(caption(ui, locale.get("ssh.failure.noSsh")).aid("settings-ssh-no-ssh"));
    }

    let machines = ssh::machines(host);
    for machine in &machines {
        let id = machine.id.clone();
        let (state, _) = ssh::state_words(host, machine);
        let to = (host.clone(), id.clone());
        let test = small(
            ui,
            locale.get("ssh.settings.test"),
            Box::new(move || {
                let (host, id) = (to.0.clone(), to.1.clone());
                crate::native::act(&to.0, async move {
                    ssh::test(&host, &id).await;
                    Ok(())
                });
            }),
        )
        .aid(format!("settings-ssh-test-{id}"))
        .enabled(!ssh::testing(host, &id));
        let to = (host.clone(), id.clone());
        let remove = small(
            ui,
            locale.get("ssh.settings.remove"),
            Box::new(move || {
                let (host, id) = (to.0.clone(), to.1.clone());
                crate::native::act(&to.0, async move {
                    ssh::remove(&host, &id).await;
                    Ok(())
                });
            }),
        )
        .aid(format!("settings-ssh-remove-{id}"));
        let buttons = El::new(Kind::Panel { children: vec![test, remove] });
        let words = ui.settings_titled(machine.target.clone(), state);
        rows.add(ui.settings_labeled(words, buttons, true));

        let label = locale.get("ssh.settings.tmux");
        let to = (host.clone(), id.clone());
        let switch = ui
            .settings_switch(&label, machine.uses_tmux(), Some(&format!("settings-ssh-tmux-{id}")), move |on| {
                ssh::set_tmux(&to.0, &to.1, on)
            })
            .enabled(machine.has_tmux);
        let help = locale.get(if machine.has_tmux { "ssh.settings.tmuxHelp" } else { "ssh.settings.tmuxMissing" });
        rows.add(ui.settings_labeled(ui.settings_titled(&label, help), switch, true));

        let typed = ui.local(&format!("ssh-dir-{id}"), || "~".to_string());
        let into = typed.clone();
        let field = ui
            .settings_field(El::text_box(typed.get()).placeholder("~/src/app").on_text(move |text| into.set(text)), 11.0)
            .width(FIELD_WIDTH)
            .named(locale.get("ssh.settings.folder"))
            .aid(format!("settings-ssh-dir-{id}"));
        let (to, dir) = ((host.clone(), id.clone()), typed.clone());
        let open = small(
            ui,
            locale.get("ssh.settings.open"),
            Box::new(move || {
                let (host, id, dir) = (to.0.clone(), to.1.clone(), dir.get());
                crate::native::act(&to.0, async move { ssh::open_folder(&host, &id, &dir).await });
            }),
        )
        .aid(format!("settings-ssh-open-{id}"))
        .enabled(mighty_core::ssh::valid_remote_dir(typed.get().trim()));
        rows.add(El::stack(true, metrics::SPACING_SM, vec![field, open]));
    }

    // Adding a machine.
    rows.add(sub_heading(ui, locale.get("ssh.settings.add")));
    let aliases: Vec<String> = ssh::aliases(host).into_iter().filter(|a| !machines.iter().any(|m| &m.target == a)).collect();
    if !aliases.is_empty() {
        let items: Vec<MenuItem> = aliases
            .iter()
            .map(|alias| {
                let (host, alias) = (host.clone(), alias.clone());
                MenuItem::item(alias.clone(), move || {
                    let message = ssh::add(&host, &alias).err();
                    ssh::say(&host, message);
                })
            })
            .collect();
        let picker = ui.settings_menu_button(locale.get("ssh.settings.fromConfig"), items).aid("settings-ssh-config-hosts");
        rows.add(ui.settings_labeled(ui.settings_text(locale.get("ssh.settings.configHosts"), TextStyle::size(12.0)), picker, false));
    }
    let typed = ui.local("ssh-target", String::new);
    let into = typed.clone();
    let field = ui
        .settings_field(El::text_box(typed.get()).placeholder("user@host:22").on_text(move |text| into.set(text)), 11.0)
        .width(FIELD_WIDTH)
        .named(locale.get("ssh.settings.add"))
        .aid("settings-ssh-target");
    let (to, adding) = (host.clone(), typed.clone());
    let add = small(
        ui,
        locale.get("ssh.settings.addButton"),
        Box::new(move || {
            let outcome = ssh::add(&to, &adding.get());
            if outcome.is_ok() {
                adding.set(String::new());
            }
            ssh::say(&to, outcome.err());
        }),
    )
    .aid("settings-ssh-add")
    .enabled(!typed.get().trim().is_empty() && machines.len() < ssh::MAX_MACHINES);
    rows.add(El::stack(true, metrics::SPACING_SM, vec![field, add]));
    rows.add(caption(ui, locale.get("ssh.settings.note")));
    if let Some(message) = ssh::message(host) {
        rows.add(caption(ui, message).aid("settings-ssh-message"));
    }
    rows.finish()
}
