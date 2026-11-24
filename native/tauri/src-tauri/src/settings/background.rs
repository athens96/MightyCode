//! 백그라운드 작업 (MC-3), in 일반 after the agents' box: the switch that keeps work running after the app closes (on unless switched off),
//! the daemon's state (its process, what runs, the output it keeps) and 데몬 중지; then the headless server's switch, 로그인할 때 창
//! 없이 시작 (a LaunchAgent or the Run key, off unless set; server/login.rs), and how this app runs now. Built from the daemon
//! module's memory and the login item's file or value only (a status older than a moment is read again on a task, then this box
//! redraws); every change runs on a task.

use std::sync::Arc;

use mighty_native::kit::{ControlSize, TextStyle};
use mighty_native::ui::{El, Ui};

use super::caption;
use crate::host::Host;

pub fn background(host: &Arc<Host>, ui: &mut Ui) -> El {
    let locale = ui.locale();
    let mut rows = ui.settings_rows();
    let label = locale.get("daemon.settings.toggle");
    let to = host.clone();
    let switch = ui
        .settings_switch(&label, crate::daemon::enabled(host), Some("settings-daemon-keep-running"), move |on| {
            crate::daemon::set_enabled(&to, on)
        })
        .enabled(host.may_change_machine());
    let words = ui.settings_titled(&label, locale.get("daemon.settings.description"));
    rows.add(ui.settings_labeled(words, switch, true));

    let to = host.clone();
    let stop = ui
        .settings_push(
            ui.button(
                locale.get("daemon.settings.stop"),
                Box::new(move || {
                    let host = to.clone();
                    crate::native::act(&to, async move {
                        crate::daemon::stop(&host).await;
                        Ok(())
                    });
                }),
            ),
            ControlSize::Small,
            false,
            false,
        )
        .aid("settings-daemon-stop")
        .enabled(crate::daemon::stoppable());
    let state = crate::daemon::status_words(host);
    rows.add(ui.settings_labeled(ui.settings_text(state, TextStyle::size(12.0)).aid("settings-daemon-status"), stop, false));
    rows.add(caption(ui, locale.get("daemon.settings.note")));

    // MC-3's headless server: started by the OS at login, without a window.
    let label = locale.get("server.settings.loginToggle");
    let to = host.clone();
    let login = ui
        .settings_switch(&label, crate::server::login::installed(host), Some("settings-server-start-at-login"), move |on| {
            let host = to.clone();
            crate::native::act(&to, async move {
                let result = crate::server::login::set(&host, on);
                crate::native::touch(&host, crate::settings::SURFACE);
                result
            });
        })
        .enabled(host.may_change_machine());
    let words = ui.settings_titled(&label, locale.get("server.settings.loginDescription"));
    rows.add(ui.settings_labeled(words, login, true));
    rows.add(caption(ui, server_state(host)).aid("settings-server-state"));
    rows.finish()
}

/// How this app runs now: started without a window (its pid and how long), or with its window.
fn server_state(host: &Host) -> String {
    let locale = host.locale;
    if crate::server::started_headless() {
        let minutes = crate::server::uptime_seconds() / 60;
        let uptime = format!("{}:{:02}", minutes / 60, minutes % 60);
        locale.get_with("server.settings.stateHeadless", &[("pid", &std::process::id().to_string()), ("uptime", &uptime)])
    } else {
        locale.get("server.settings.stateWindowed")
    }
}
