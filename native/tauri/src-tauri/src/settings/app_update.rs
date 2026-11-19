//! 앱 업데이트 (C# `BuildAppUpdateSection` and `RenderAppUpdateSection`, MainWindow.AppUpdate.cs:104-255; M/AppUpdateSettingsView.swift)
//! over mighty-core's `app_update_view::describe` of the live coordinator (app_update.rs). Owner: W9-C.
//!
//! The version beside its label (the one place Settings shows it); the automatic switch with its description; a notice row a build
//! without a key shows the state in; the state (the ring while checking, staging or installing, the 140-wide bar while downloading, the
//! words) beside the one button; the failure row (a kept error beside a staged update, the retry promise); the release notes; then,
//! folded under 고급 since they are a developer's, the typed address (with its hint, or the build's address line) or the built-in
//! address under its label and the 10pt signature note — open the first time when an address was typed. Rows C# collapses are hidden
//! (the page drops them). The box is built again on every change of the update (app_update.rs `Seams::changed`).
//!
//! The button checks, downloads, cancels or restarts only where the build may change the machine (plan §4: a development build or a
//! `--read-only` launch never writes `<data>\updates`); elsewhere it is drawn disabled.

use std::sync::Arc;

use mighty_core::app_update_view::{Phase, State, describe, text, text_in};
use mighty_core::design_tokens::{Token, metrics};
use mighty_native::kit::{ControlSize, TextStyle};
use mighty_native::ui::{Content, El, Grid, Kind, Track, Ui};

use super::folded;
use crate::host::Host;

/// C#'s automation ids.
pub const STATUS_ID: &str = "app-update-status";
pub const BUTTON_ID: &str = "app-update-action";
pub const TOGGLE_ID: &str = "app-update-automatic";
pub const ADDRESS_ID: &str = "app-update-url";
pub const RETRY_ID: &str = "app-update-retry";

/// C#'s `MaxLines` of the release notes and of the kept error.
const NOTES_LINES: usize = 8;
const KEPT_ERROR_LINES: usize = 3;

/// C#'s `MaxLines` with `CharacterEllipsis`, as far as the page can follow it (it has no line clamp): the first `lines` lines of the
/// text, "…" after them when more followed. A long line the box wraps counts once here, where C# counts each wrapped line.
fn first_lines(text: &str, lines: usize) -> String {
    let mut all = text.lines();
    let kept: Vec<&str> = all.by_ref().take(lines).collect();
    let mut shown = kept.join("\n");
    if all.next().is_some() {
        shown.push('…');
    }
    shown
}

pub fn section(host: &Arc<Host>, ui: &mut Ui) -> El {
    let locale = ui.locale();
    let update = crate::app_update::of(host);
    let (view, phase, fraction, press_allowed) = match &update {
        Some(update) => {
            let state = update.core.coordinator.state();
            (update.core.describe(&locale), state.phase, state.download_fraction, update.core.press_allowed())
        }
        // Before the app update exists (never once main.rs started it): a build that cannot check.
        None => {
            let view =
                describe(&locale, crate::app_update::words(), &State::default(), false, None, |at| at.format("%H:%M").to_string(), true);
            (view, Phase::Idle, 0.0, false)
        }
    };
    let (typed_url, automatic) = {
        let view = host.view();
        (view.snapshot.app_update_manifest_url_override.clone(), view.snapshot.app_update_auto_check)
    };
    let mut rows = ui.settings_rows();
    let version = locale.get_with("settings.appUpdate.betaVersionTemplate", &[("version", &crate::settings::about::version_text(host))]);
    let label = ui.settings_text(text(&locale, "CurrentVersionLabel"), TextStyle::default());
    rows.add(ui.settings_labeled(label, ui.settings_text(version, TextStyle::size(13.0).ink(Token::Ink2).selectable()), false));

    // The automatic switch, what it does under its label.
    let toggle_label = text(&locale, "AutoUpdateToggle");
    let to = host.clone();
    let switch = ui.settings_switch(&toggle_label, automatic, Some(TOGGLE_ID), move |on| {
        let host = to.clone();
        crate::native::act(&to, async move { crate::app_update::set_automatic(&host, on) });
    });
    let words = ui.settings_titled(&toggle_label, text(&locale, "AutoUpdateDescription"));
    rows.add(ui.settings_labeled(words, switch, true)).visible = view.section_enabled;

    // The state's ink: accent and medium for a new version, the wait ink for a failure or a build that cannot check, the primary ink
    // while it works, the quiet ink otherwise.
    let working = view.section_enabled && matches!(phase, Phase::Checking | Phase::Staging | Phase::Installing);
    let downloading = view.section_enabled && phase == Phase::Downloading;
    let ink = if !view.section_enabled || phase == Phase::Failed {
        Token::WaitText
    } else if phase == Phase::Available {
        Token::Accent
    } else if working || downloading {
        Token::Ink
    } else {
        Token::Ink2
    };
    let mut style = TextStyle::size(11.0).ink(ink).selectable();
    style.medium = view.section_enabled && phase == Phase::Available;
    let status = ui.settings_text(view.status_text.clone(), style).aid(STATUS_ID);
    // A build with no key says so on a row of its own, over the disabled button; the words move there and back.
    let alone = !view.section_enabled;
    let mut cells = vec![
        El::new(Kind::ProgressRing { active: working }).width(14.0).height(14.0).visible(working),
        El::new(Kind::ProgressBar { value: fraction, max: 1.0, indeterminate: false }).width(140.0).visible(downloading).cell(0, 1),
    ];
    if alone {
        rows.add(status);
    } else {
        rows.add(El::grid(Grid::default())).visible = false;
        cells.push(status.cell(0, 2));
    }
    let state = El::grid(Grid { children: cells, cols: vec![Track::Auto, Track::Auto, Track::Star(1.0)], ..Grid::default() });
    let to = host.clone();
    let button = El::button(Content::Text(view.button_label.clone()))
        .named(view.button_label.clone())
        .enabled(view.button_enabled && press_allowed)
        .aid(BUTTON_ID)
        .on_click(move || crate::app_update::press(&to));
    let button = ui.settings_push(button, ControlSize::Regular, false, false);
    rows.add(ui.settings_labeled(state, button, false));

    // A failed check shows only here: the error beside a staged update, which stays ready, and the promise to try again.
    let mut failure = vec![];
    if let Some(kept) = &view.kept_error {
        failure.push(ui.settings_text(first_lines(kept, KEPT_ERROR_LINES), TextStyle::size(11.0).ink(Token::WaitText).selectable()));
    }
    if let Some(retry) = &view.retry_notice {
        failure.push(ui.settings_text(retry.clone(), TextStyle::size(10.0).ink(Token::Ink2)));
    }
    let shown = view.section_enabled && !failure.is_empty();
    rows.add(El::stack(false, metrics::SPACING_XS, failure).aid(RETRY_ID)).visible = shown;

    let notes = view.notes.as_deref().map(|notes| first_lines(notes, NOTES_LINES)).unwrap_or_default();
    rows.add(ui.settings_text(notes, TextStyle::size(11.0).ink(Token::Ink2).selectable())).visible = view.notes.is_some();

    // 고급, folded: the typed address, saved as it is typed (C#'s TextChanged), with its hint or the build's address line under it, or
    // the build's own address under its label (screens/10-settings-about-*.webp); then the signature note.
    let typed_before = typed_url.as_deref().is_some_and(|url| !url.is_empty());
    let to = host.clone();
    let address = ui
        .settings_field(El::text_box(typed_url.unwrap_or_default()), 12.0)
        .placeholder(text(&locale, "ManifestUrlPlaceholder"))
        .aid(ADDRESS_ID)
        .enabled(view.address_field_enabled && view.section_enabled)
        .on_text(move |typed| {
            let _ = to.view().update(|state| state.app_update_manifest_url_override = (!typed.is_empty()).then_some(typed));
        });
    let hint = view.built_in_address_line.clone().unwrap_or_else(|| text_in(&locale, crate::app_update::words(), "ManifestUrlHint"));
    let typed = El::stack(false, metrics::SPACING_SM, vec![address, ui.settings_text(hint, TextStyle::size(11.0).ink(Token::Ink2))])
        .visible(view.address_field_enabled);
    let built_in = update.as_ref().and_then(|u| u.core.coordinator.built_in_manifest_url().map(str::to_string)).unwrap_or_default();
    let mut address_text = TextStyle::size(11.0).ink(Token::Ink2).selectable();
    address_text.mono = true;
    let built_in = El::stack(
        false,
        metrics::SPACING_XS,
        vec![
            ui.settings_text(locale.get("settings.appUpdate.manifestUrlAddressLabel"), TextStyle::default()),
            ui.settings_text(built_in, address_text),
        ],
    )
    .visible(!view.address_field_enabled);
    let signature = ui.settings_text(view.signature_notice.clone(), TextStyle::size(10.0).ink(Token::Ink2));
    let body = El::stack(false, metrics::SPACING_MD, vec![typed, built_in, signature]);
    let advanced = folded(ui, "app-update-advanced", typed_before, locale.get("settings.advancedTitle"), body);
    rows.add(advanced).visible = view.section_enabled;
    rows.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// C#'s 8 and 3 lines: what is longer is cut after its last kept line with "…"; what fits is left as it is.
    #[test]
    fn the_notes_and_the_kept_error_keep_csharps_line_counts() {
        assert_eq!(first_lines("a\nb\nc", KEPT_ERROR_LINES), "a\nb\nc");
        assert_eq!(first_lines("a\nb\nc\nd", KEPT_ERROR_LINES), "a\nb\nc…");
        let notes: Vec<String> = (1..=10).map(|n| format!("line {n}")).collect();
        assert_eq!(first_lines(&notes.join("\n"), NOTES_LINES), format!("{}…", notes[..8].join("\n")));
        assert_eq!(first_lines("one long line", NOTES_LINES), "one long line");
    }
}
