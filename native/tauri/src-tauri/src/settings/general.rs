//! `general`: 화면, 글자 and 에이전트 창 (C# MainWindow.Settings.cs:83-142 `BuildDisplaySection`, `BuildAgentLinksSection`;
//! MainWindow.TextAppearance.cs:113-202 `BuildTextSection`; MainWindow.Delegation.cs `DelegationOn`, `SetDelegationOn`). Owner: W6-A.
//!
//! 화면 is the app's look only (theme, language); what changes how agent panes run — Claude's status line, Mighty orchestration — and
//! the panes' keyboard tips (which 앱 정보 used to hold) are 에이전트 창's.

use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use mighty_core::design_tokens::{Token, metrics};
use mighty_core::locale::LANGUAGES;
use mighty_core::text_appearance::{SCALES, TERMINAL_BASE_SIZE, TextAppearance, code_families};
use mighty_native::kit::{ControlSize, SETTINGS_POPUP_CHIP_INSET, TextStyle};
use mighty_native::ui::{Border, Content, El, HAlign, Kind, TextBlock, Thickness, Ui, Wrapping};
use mighty_platform::{PrefValue, PrefsStore};
use tauri::Manager;

use crate::host::Host;

/// `Locale.PickerChoices`: the language picker's values and their labels' keys.
pub const LANGUAGE_CHOICES: [(&str, &str); 5] = [
    ("system", "settings.display.languageSystem"),
    ("ko", "settings.display.languageKorean"),
    ("en", "settings.display.languageEnglish"),
    ("zh", "settings.display.languageChinese"),
    ("ja", "settings.display.languageJapanese"),
];

/// `DelegationSwitch`'s value name under `HKCU\Software\MightyCode`.
pub const DELEGATION_VALUE: &str = "delegation.enabled";

/// How long a reading of the registry value stands (C# `DelegationSwitch.ReadFor`: a builder asks four times a second).
const DELEGATION_READ_FOR: Duration = Duration::from_secs(2);

/// The delegation switch as the app holds it (C# `DelegationSwitch`'s `last`, plus what a run that may not write holds).
#[derive(Debug, Default)]
struct Delegation {
    /// The value switched in this run when it may not be written (a smoke's own, C# `smokeDelegationOn`; else this run's).
    held: Option<bool>,
    /// The registry's value and when it was read (or written).
    reading: Option<(bool, Instant)>,
}

impl Delegation {
    /// The value without reading the registry: this run's, else a reading under 2 s old.
    fn known(&self, now: Instant) -> Option<bool> {
        self.held.or(self.reading.filter(|(_, at)| now.saturating_duration_since(*at) < DELEGATION_READ_FOR).map(|(on, _)| on))
    }

    /// `Set`'s outcome: a written value is the reading; a refused write forgets the reading, so the switch shows the registry's value
    /// again (C#: `last = null; return false`).
    fn written(&mut self, on: bool, saved: bool, now: Instant) {
        self.reading = if saved { Some((on, now)) } else { None };
    }
}

static DELEGATION: Mutex<Delegation> = Mutex::new(Delegation { held: None, reading: None });

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// C# `DelegationOn`: a smoke's switch (off until it is switched), else this run's, else the registry's from a reading at most 2 s old
/// (a missing value is on).
pub fn delegation_on(host: &Host) -> bool {
    let now = Instant::now();
    let known = lock(&DELEGATION).known(now);
    if let Some(on) = known {
        return on;
    }
    if host.smoke.is_some() {
        return false;
    }
    // Read off the lock; a registry that cannot be read is on, as C# takes it.
    let saved = host.app.try_state::<Arc<dyn PrefsStore>>().and_then(|prefs| prefs.get(DELEGATION_VALUE).ok().flatten());
    let on = saved.is_none_or(|value| value.is_on());
    lock(&DELEGATION).reading = Some((on, now));
    on
}

/// C# `SetDelegationOn`: the registry's DWORD with `--allow-state-write` (a refused write puts the registry's value back on the
/// switch), else the value held for this run (a smoke's own).
pub fn set_delegation_on(host: &Host, on: bool) {
    match host.app.try_state::<Arc<dyn PrefsStore>>().filter(|_| host.may_change_machine()) {
        Some(prefs) => {
            let saved = prefs.set(DELEGATION_VALUE, PrefValue::Int(i64::from(on)));
            if let Err(error) = &saved {
                host.log.line(&format!("delegation switch not saved: {error}"));
            }
            lock(&DELEGATION).written(on, saved.is_ok(), Instant::now());
        }
        None => lock(&DELEGATION).held = Some(on),
    }
    // C# `SetDelegationOn`: every running pane learns again whether its folder can host a child worktree (delegation.rs, W7-D).
    crate::delegation::switch_changed(host);
}

/// `InstalledFamilies`: every installed family and the code picker's (`CodeFamilies`), read once per run from the font catalog
/// ([`read_families`] when Settings opens; none until then, as when the OS could not list them).
static FAMILIES: OnceLock<(Vec<String>, Vec<String>)> = OnceLock::new();

/// Reads the installed families once (I/O: called as Settings opens, never from a builder).
pub fn read_families(host: &Host) {
    if FAMILIES.get().is_some() {
        return;
    }
    let read = host.app.try_state::<crate::platform::Ports>().and_then(|ports| ports.fonts.families().ok()).unwrap_or_default();
    let all: Vec<String> = read.iter().map(|f| f.name.clone()).collect();
    let code = code_families(read.iter().map(|f| (f.name.as_str(), f.monospaced)));
    let _ = FAMILIES.set((all, code));
}

fn families() -> (Vec<String>, Vec<String>) {
    FAMILIES.get().cloned().unwrap_or_default()
}

/// 화면 (C# `BuildDisplaySection`): the theme and language pickers and the language note (not C#'s browser switch: 0.5 has no in-app
/// browser).
pub fn display(host: &Arc<Host>, ui: &mut Ui) -> El {
    let locale = ui.locale();
    let (theme, preference) = {
        let view = host.view();
        let s = &view.snapshot;
        let theme = if s.theme() == "light" { "light" } else { "dark" };
        (theme, s.language_preference().to_string())
    };
    let mut rows = ui.settings_rows();

    let theme_label = locale.get("settings.display.themeLabel");
    let options = [
        ("dark".to_string(), locale.get("settings.display.themeDarkWindows")),
        ("light".to_string(), locale.get("settings.display.themeLightWindows")),
    ];
    let to = host.clone();
    let picker = ui.settings_segmented(&theme_label, "settings-theme", &options, theme, move |value| set_theme(&to, &value), false, true);
    rows.add(ui.settings_labeled(ui.settings_text(&theme_label, TextStyle::default()), picker, false));

    let language_label = locale.get("settings.display.languageLabel");
    let options: Vec<(String, String)> = LANGUAGE_CHOICES.iter().map(|(value, key)| (value.to_string(), locale.get(key))).collect();
    let chosen = if LANGUAGES.contains(&preference.as_str()) { preference.as_str() } else { "system" };
    let to = host.clone();
    let picker = ui.settings_segmented(
        &language_label,
        "settings-language",
        &options,
        chosen,
        move |value| {
            crate::native::act(&to, {
                let to = to.clone();
                async move { to.view().update(|state| state.language_preference = Some(value)) }
            })
        },
        false,
        true,
    );
    rows.add(ui.settings_labeled(ui.settings_text(&language_label, TextStyle::default()), picker, false));
    rows.add(ui.settings_text(locale.get("settings.display.languageRestartNote"), TextStyle::size(11.0).ink(Token::Ink2)));
    rows.finish()
}

/// 에이전트 창: the status line and delegation switches, each with its explanation under its label, then the panes' keyboard tips
/// (C# `BuildAppInfoSection`'s Windows note; the Mac's own words name its login shell, not PowerShell).
pub fn agents(host: &Arc<Host>, ui: &mut Ui) -> El {
    let locale = ui.locale();
    let status_line = host.view().snapshot.status_line_enabled;
    let mut rows = ui.settings_rows();
    let label = locale.get("settings.display.statusLineToggle");
    let to = host.clone();
    let switch = ui.settings_switch(&label, status_line, Some("settings-status-line"), move |on| set_status_line(&to, on));
    let words = ui.settings_titled(&label, locale.get("settings.display.statusLineDescription"));
    rows.add(ui.settings_labeled(words, switch, true));

    let label = locale.get("settings.delegation.toggle");
    let to = host.clone();
    let switch = ui.settings_switch(&label, delegation_on(host), Some("settings-delegation"), move |on| set_delegation_on(&to, on));
    let words = ui.settings_titled(
        &label,
        mighty_core::settings_words::text(&locale, crate::platform::words(), "settings.delegation.descriptionWindows").unwrap_or_default(),
    );
    rows.add(ui.settings_labeled(words, switch, true));

    // 에이전트에게 진행 체크리스트 요청: Antigravity, Codex, opencode and Hermes are asked to keep a plan the progress panel shows
    // (mighty_run::checklist).
    let label = locale.get("settings.agents.checklistToggle");
    let checklist = host.view().snapshot.agent_checklist_on();
    let to = host.clone();
    let switch = ui.settings_switch(&label, checklist, Some("settings-agent-checklist"), move |on| set_agent_checklist(&to, on));
    let words = ui.settings_titled(&label, locale.get("settings.agents.checklistDescription"));
    rows.add(ui.settings_labeled(words, switch, true));

    // 외부 편집기: what 편집기에서 열기 starts (editor.rs): 자동, each editor found (looked for as Settings opened), 시스템 기본.
    let label = locale.get("settings.editor.label");
    let (choices, chosen) =
        crate::editor::picker_rows(&locale, &crate::editor::choice(host), &crate::editor::detected(), crate::platform::profile().os);
    let values: Vec<String> = choices.iter().map(|(value, _)| value.clone()).collect();
    let items: Vec<Content> = choices.into_iter().map(|(_, words)| Content::from(words)).collect();
    let to = host.clone();
    let combo = picker(ui, &label, "settings-external-editor", chosen as i32, items).on_select(move |at| {
        if let Some(value) = values.get(at as usize) {
            crate::editor::set_choice(&to, value);
        }
    });
    let words = ui.settings_titled(&label, locale.get("settings.editor.description"));
    rows.add(ui.settings_labeled(words, framed(ui, combo), true));
    if let Some(tips) = mighty_core::settings_words::text(&locale, crate::platform::words(), "settings.appInfo.windowsNote") {
        rows.add(ui.settings_text(tips, TextStyle::size(11.0).ink(Token::Ink2)).aid("settings-agent-tips"));
    }
    rows.finish()
}

/// The theme picker (C#: `UpdateAsync(Theme)` then `Render`): saved like `setTheme`, the terminals recoloured.
pub fn set_theme(host: &Arc<Host>, value: &str) {
    let host = host.clone();
    let value = value.to_string();
    crate::native::act(&host.clone(), async move {
        host.view().update(|state| state.theme = Some(value))?;
        crate::terminal::restyle(&host);
        Ok(())
    });
}

/// C# `SetStatusLineEnabled`: saved, then each pane's status line read again (status_line.rs, W6-B).
fn set_status_line(host: &Arc<Host>, on: bool) {
    let host = host.clone();
    crate::native::act(&host.clone(), async move {
        host.view().update(|state| state.status_line_enabled = on)?;
        crate::status_line::set_enabled(&host, on);
        host.panes_changed();
        Ok(())
    });
}

/// "에이전트에게 진행 체크리스트 요청": saved; the next request reads it as it starts.
fn set_agent_checklist(host: &Arc<Host>, on: bool) {
    let host = host.clone();
    crate::native::act(&host.clone(), async move { host.view().update(|state| state.agent_checklist = Some(on)) });
}

/// 글자 (C# `BuildTextSection`): the size from 85% to 150%, the interface, conversation and code fonts each over a line of words in
/// that font, the shortcuts' note and the button back to the standard setting. Every choice applies at once.
pub fn text(host: &Arc<Host>, ui: &mut Ui) -> El {
    let locale = ui.locale();
    let current = crate::frame::text_setting(host);
    let (all, code) = families();
    let mut rows = ui.settings_rows();

    let size_label = locale.get("settings.text.sizeLabel");
    let sizes: Vec<Content> = SCALES
        .iter()
        .map(|scale| {
            let percent = TextAppearance::new(*scale, None, None, None).percent().to_string();
            if *scale == 1.0 {
                locale.get_with("settings.text.sizeStandard", &[("percent", &percent)]).into()
            } else {
                format!("{percent}%").into()
            }
        })
        .collect();
    let index = SCALES.iter().position(|s| *s == current.scale).unwrap_or(2) as i32;
    let to = host.clone();
    let size = picker(ui, &size_label, "settings-text-size", index, sizes).on_select(move |at| {
        if let Some(scale) = SCALES.get(at as usize) {
            crate::frame::set_text(&to, crate::frame::text_setting(&to).with_scale(*scale));
        }
    });
    rows.add(ui.settings_labeled(ui.settings_text(&size_label, TextStyle::default()), framed(ui, size), false));

    let blocks: [FontBlock; 3] = [
        (
            "settings.text.uiFontLabel",
            "settings-text-font",
            "settings.text.fontSystem",
            &all,
            current.family.as_deref(),
            TextAppearance::with_family,
            Area::Interface,
        ),
        (
            "settings.text.transcriptFontLabel",
            "settings-text-transcript-font",
            "settings.text.sameAsInterface",
            &all,
            current.transcript_family.as_deref(),
            TextAppearance::with_transcript_family,
            Area::Transcript,
        ),
        (
            "settings.text.codeFontLabel",
            "settings-text-code-font",
            "settings.text.monospaceSystem",
            &code,
            current.code_family.as_deref(),
            TextAppearance::with_code_family,
            Area::Code,
        ),
    ];
    for (label_key, aid, none_key, list, saved, change, area) in blocks {
        let label = locale.get(label_key);
        // A saved family no longer installed stays listed (it draws in the stand-in).
        let mut names: Vec<String> = list.to_vec();
        if let Some(saved) = saved
            && !names.iter().any(|n| n.eq_ignore_ascii_case(saved))
        {
            names.push(saved.to_string());
        }
        let index = saved.and_then(|s| names.iter().position(|n| n.eq_ignore_ascii_case(s))).map_or(0, |at| at + 1) as i32;
        let items: Vec<Content> = std::iter::once(locale.get(none_key)).chain(names.iter().cloned()).map(Content::from).collect();
        let (to, chosen) = (host.clone(), names.clone());
        let combo = picker(ui, &label, aid, index, items).on_select(move |at| {
            let family = (at > 0).then(|| chosen.get(at as usize - 1).cloned()).flatten();
            crate::frame::set_text(&to, change(&crate::frame::text_setting(&to), family.as_deref()));
        });
        let line = ui.settings_labeled(ui.settings_text(&label, TextStyle::default()), framed(ui, combo), false);
        let size = if area == Area::Code { TERMINAL_BASE_SIZE } else { metrics::TYPE_BODY };
        let preview = preview(ui, &current, area, size);
        rows.add(El::stack(false, metrics::SPACING_SM, vec![line, preview]));
    }

    let to = host.clone();
    let reset = ui
        .settings_push(
            ui.button(locale.get("settings.text.reset"), move || crate::frame::set_text(&to, TextAppearance::default())),
            ControlSize::Regular,
            false,
            false,
        )
        .aid("settings-text-reset")
        .enabled(!current.is_standard());
    let note = ui.settings_text(
        mighty_core::settings_words::text(&locale, crate::platform::words(), "settings.text.noteWindows").unwrap_or_default(),
        TextStyle::size(11.0).ink(Token::Ink2),
    );
    rows.add(ui.settings_labeled(note, reset, false));
    rows.finish()
}

/// One font picker of the text box: its label's key, its automation id, its "none" entry's key, the families it lists, the family
/// saved, how a pick changes the setting, and the area its preview draws in.
type FontBlock<'a> =
    (&'static str, &'static str, &'static str, &'a [String], Option<&'a str>, fn(&TextAppearance, Option<&str>) -> TextAppearance, Area);

/// Which of the text setting's fonts a preview draws in (C# `TextArea`).
#[derive(Clone, Copy, PartialEq, Eq)]
enum Area {
    Interface,
    Transcript,
    Code,
}

/// The text box's pop-up (C#'s `Picker`): an inline pop-up at the right, at most 240 wide, named and marked.
fn picker(ui: &Ui, label: &str, aid: &str, index: i32, items: Vec<Content>) -> El {
    let mut combo = ui.settings_popup(El::combo_box(index, items), false).ha(HAlign::Right).named(label).aid(aid);
    combo.max_width = Some(240.0);
    combo
}

/// C#'s `Framed`: the pop-up with the Mac's chip, ending at the row's edge.
fn framed(ui: &Ui, combo: El) -> El {
    ui.settings_popup_frame(combo).margin(Thickness::new(0.0, 0.0, -SETTINGS_POPUP_CHIP_INSET, 0.0))
}

/// C#'s `Preview`: the preview words in the area's font on the subtle wash.
fn preview(ui: &Ui, text: &TextAppearance, area: Area, size: f64) -> El {
    let family = match area {
        Area::Interface => Some(text.family.clone().unwrap_or_else(|| metrics::FONT_BODY.to_string())),
        Area::Transcript => text.resolved_transcript_family().map(str::to_string),
        Area::Code => Some(text.code_family.clone().unwrap_or_else(|| metrics::FONT_MONO.to_string())),
    };
    let words = El::new(Kind::TextBlock(TextBlock {
        text: ui.locale().get("settings.text.preview"),
        size: Some(size),
        weight: 400,
        family,
        foreground: Some(ui.brush(Token::Ink)),
        wrapping: Wrapping::Wrap,
        ..TextBlock::default()
    }))
    .aid(match area {
        Area::Interface => "settings-text-preview",
        Area::Transcript => "settings-text-transcript-preview",
        Area::Code => "settings-text-code-preview",
    });
    El::border(Border {
        child: Some(Box::new(words)),
        padding: Thickness::all(metrics::SPACING_SM),
        corner_radius: metrics::RADIUS_SEGMENT,
        background: Some(ui.env.theme.subtle()),
        ..Border::default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Review L6: a refused write puts the registry's value back on the switch (the reading is forgotten, so the next ask reads the
    /// registry), a reading stands 2 s and no longer, and a value held for this run wins over any reading.
    #[test]
    fn the_delegation_switch_follows_the_registry() {
        let start = Instant::now();
        let mut switch = Delegation { held: None, reading: Some((true, start)) };
        assert_eq!(switch.known(start + Duration::from_millis(1999)), Some(true));
        assert_eq!(switch.known(start + DELEGATION_READ_FOR), None, "read again after 2 s");
        switch.written(false, false, start);
        assert_eq!(switch.known(start), None, "a refused write is not the value: the registry is read again");
        switch.written(false, true, start);
        assert_eq!(switch.known(start), Some(false), "a written value is the reading");
        switch.held = Some(true);
        assert_eq!(switch.known(start + Duration::from_secs(60)), Some(true));
    }
}
