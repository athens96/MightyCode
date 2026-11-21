//! `about`: 앱 업데이트 and 앱 정보 (C# MainWindow.AppUpdate.cs:110-255, MainWindow.Settings.cs:1031-1045). Owner: W6-A.

use std::sync::Arc;

use mighty_core::design_tokens::Token;
use mighty_native::kit::{ControlSize, TextStyle};
use mighty_native::ui::{El, Ui};

use crate::host::Host;

/// `AppVersionText`: the build's version without its `+…` stamp.
pub fn version_text(host: &Host) -> String {
    host.version.split('+').next().unwrap_or_default().to_string()
}

/// 앱 업데이트 (C# `BuildAppUpdateSectionFromState`) for a build with no update source (W9 brings the updates).
pub fn app_update(host: &Arc<Host>, ui: &mut Ui) -> El {
    crate::settings::app_update::section(host, ui)
}

/// 앱 정보 (C# `BuildAppInfoSection`): the guide's button, the state folder's button (Finder on the Mac, Explorer on Windows) and its
/// path in 10pt mono. The version is 앱 업데이트's first row, shown once; the keyboard tips are 일반 › 에이전트 창's.
pub fn app_info(host: &Arc<Host>, ui: &mut Ui) -> El {
    let locale = ui.locale();
    let mut rows = ui.settings_rows();
    let to = host.clone();
    let help = ui
        .settings_push(
            ui.button(locale.get("settings.appInfo.openHelpButton"), move || {
                let host = to.clone();
                crate::native::act(&to, async move { crate::settings::open_help(&host).await });
            }),
            ControlSize::Regular,
            false,
            false,
        )
        .aid("settings-open-help");
    rows.add(ui.settings_labeled(ui.settings_text(locale.get("menu.help"), TextStyle::default()), help, false));
    let folder = host.data_dir.clone();
    let to = host.clone();
    let show = ui
        .settings_push(
            ui.button(
                mighty_core::settings_words::text(&locale, crate::platform::words(), "menu.showInExplorer").unwrap_or_default(),
                move || {
                    let folder = folder.clone();
                    crate::native::act(&to, async move {
                        tauri::async_runtime::spawn_blocking(move || crate::platform::show_in_explorer(&folder))
                            .await
                            .map_err(|error| error.to_string())?
                    });
                },
            ),
            ControlSize::Regular,
            false,
            false,
        )
        .aid("settings-state-folder");
    rows.add(ui.settings_labeled(ui.settings_text(locale.get("settings.appInfo.stateLocationLabel"), TextStyle::default()), show, false));
    let path = host.data_dir.to_string_lossy().into_owned();
    rows.add(ui.settings_text(path, TextStyle::size(10.0).ink(Token::Ink2).mono().selectable()));
    rows.finish()
}
