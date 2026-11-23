//! 백그라운드 작업 (MC-3), in 일반 after the agents' box: the switch that keeps work running after the app closes (off unless set),
//! the daemon's state (its process, what runs, the output it keeps) and 데몬 중지. Built from the daemon module's memory only (a
//! status older than a moment is read again on a task, then this box redraws); every change runs on a task.

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
    rows.finish()
}
