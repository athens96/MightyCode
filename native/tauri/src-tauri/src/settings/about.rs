//! `about`: 앱 업데이트 and 앱 정보 (C# MainWindow.AppUpdate.cs:110-255, MainWindow.Settings.cs:1031-1045). Owner: W6-A.

use std::sync::Arc;

use mighty_core::design_tokens::{Token, metrics};
use mighty_native::kit::{ControlSize, TextStyle};
use mighty_native::ui::{El, Kind, Ui};

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

/// MC-54: 진단: what the bundle holds, 진단 묶음 내보내기 (the save dialog, then the zip made off the async workers; disabled with a ring
/// while one is being made), then where the last one went with 파인더에서 보기, or why it failed.
pub fn diagnostics(host: &Arc<Host>, ui: &mut Ui) -> El {
    let locale = ui.locale();
    let status = crate::diagnostics::status();
    let mut rows = ui.settings_rows();
    rows.add(crate::settings::caption(ui, locale.get("diagnostics.settings.description")));
    let to = host.clone();
    let label = if status.running { locale.get("diagnostics.settings.exporting") } else { locale.get("diagnostics.settings.export") };
    let export = ui
        .settings_push(
            ui.button(label, move || {
                let host = to.clone();
                crate::native::act(&to, crate::diagnostics::export(host));
            })
            .enabled(!status.running),
            ControlSize::Regular,
            false,
            false,
        )
        .aid("settings-diagnostics-export");
    let ring = El::new(Kind::ProgressRing { active: status.running }).width(14.0).height(14.0).visible(status.running);
    let title = ui.settings_titled(locale.get("diagnostics.settings.label"), locale.get("diagnostics.settings.privacy"));
    rows.add(ui.settings_labeled(El::stack(true, metrics::SPACING_SM, vec![title, ring]), export, true));
    if let Some(saved) = status.saved.as_ref().filter(|_| !status.running && status.error.is_none()) {
        let name = saved.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let words = locale.get_with(
            "diagnostics.settings.saved",
            &[("name", &name), ("files", &saved.files.to_string()), ("redacted", &saved.redacted.to_string())],
        );
        let to = host.clone();
        let show = ui
            .settings_push(
                ui.button(
                    mighty_core::settings_words::text(&locale, crate::platform::words(), "menu.showInExplorer").unwrap_or_default(),
                    move || {
                        let host = to.clone();
                        crate::native::act(&to, crate::diagnostics::show_saved(host));
                    },
                ),
                ControlSize::Regular,
                false,
                false,
            )
            .aid("settings-diagnostics-show");
        rows.add(ui.settings_labeled(ui.settings_text(words, TextStyle::size(11.0).ink(Token::Ink2).selectable()), show, false));
    }
    if let Some(error) = status.error.as_ref().filter(|_| !status.running) {
        rows.add(
            ui.settings_text(error.clone(), TextStyle::size(11.0).ink(Token::WaitText).selectable()).aid("settings-diagnostics-error"),
        );
    }
    rows.finish()
}
