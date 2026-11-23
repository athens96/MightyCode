//! Settings in the page (C# MainWindow.WebShell.Settings.cs `ShowWebSettingsAsync`, `SelectWebSettingsCategory`, `CloseWebSettings`;
//! MainWindow.Settings.cs:61-77 `SettingsGroups`; MainWindow.SettingsNavigation.cs `SettingsCategoryGlyph`), `openSettings` and
//! `openHelp` (MainWindow.Help.cs `OpenHelp`). Owner: W6-A.
//!
//! The surface `settings` (place `surface`): the available categories with their symbols, the one chosen — the saved `SettingsPane`
//! when it is available, else the first —, and that category's boxes in the Mac's order, each the rows its builder makes (every box
//! has its own file, by its owner) toned as `BuildSectionContainer` tones them. It holds the window's dialog while it is open (C#
//! `dialogOpen`) and refuses to open while another sheet is up. A nav saves `SettingsPane` (kept in memory; written only with
//! `--allow-state-write`); 구성 요소 reads the account statuses again, except in a smoke. A saved category of 0.5.3 and earlier that
//! has no tab any more opens where its boxes are now (모바일 리모트: 원격 연결's 휴대폰 tab; CLI: 구성 요소).

pub mod about;
pub mod app_update;
pub mod automation;
pub mod background;
pub mod companion;
pub mod general;
pub mod jobs;
pub mod mobile;
pub mod mobile_direct;
pub mod models;
pub mod remote;
pub mod styles;
pub mod tools;

use std::sync::Arc;

use mighty_core::design_tokens::{Token, metrics};
use mighty_core::help_site;
use mighty_core::locale::Locale;
use mighty_core::settings_sections::{self as sections, available};
use mighty_native::kit::TextStyle;
use mighty_native::surface::{Next, SurfaceModel};
use mighty_native::ui::{Content, El, Ui};
use mighty_native::view::{NavItem, Place, Section, View};
use serde_json::Value;

use crate::frame::Sheet;
use crate::host::{Host, Registry};

/// The surface's id (the page's `data-native-view`).
pub const SURFACE: &str = "settings";

/// A box's builder: its rows from the host's models, no I/O (it runs on every read).
pub type Builder = fn(&Arc<Host>, &mut Ui) -> El;

/// One box of a category (C# `SettingsGroup`): its heading, its builder, whether the 베타 capsule follows the heading.
pub struct Group {
    pub title: String,
    pub build: Builder,
    pub beta: bool,
}

/// `SettingsGroups(category)`: each category's boxes in the tabs' order. A registered slot keeps the heading Core gives it; the boxes
/// drawn inside a slot's tab (the text, the agent panes' switches, the toolkit) take theirs from the catalogue. The paused screen view
/// box (one sentence, nothing to set) is no longer drawn: it comes back with screen view.
pub fn groups(locale: &Locale, category: &str) -> Vec<Group> {
    let slot = |id: &str, build: Builder| Group { title: sections::title_of(locale, id).unwrap_or_default(), build, beta: false };
    let titled = |key: &str, build: Builder| Group { title: locale.get(key), build, beta: false };
    match category {
        "general" => vec![
            slot(sections::DISPLAY, general::display),
            titled("settings.text.sectionTitle", general::text),
            titled("settings.agents.sectionTitle", general::agents),
            titled("daemon.settings.sectionTitle", background::background),
        ],
        "models" => vec![slot(sections::PHASE_MODELS, models::phase_models)],
        "styles" => vec![slot(sections::STYLES, styles::styles)],
        "remote" => vec![slot(sections::REMOTE_WORKSPACES, remote::remote_workspaces)],
        "companion" => {
            vec![
                slot(sections::COMPANION, companion::companion),
                titled("automation.settings.sectionTitle", automation::automation),
                titled("automation.jobs.sectionTitle", jobs::jobs),
            ]
        }
        "tools" => vec![slot(sections::COMPONENTS, tools::components), titled("settings.toolkit.sectionTitle", tools::toolkit)],
        "about" => vec![slot(sections::APP_UPDATE, about::app_update), slot(sections::APP_INFO, about::app_info)],
        _ => vec![],
    }
}

/// A part's heading inside a box (원격 연결's parts, the phone's groups): one size for every tab, 13pt medium.
pub(crate) fn sub_heading(ui: &Ui, title: impl Into<String>) -> El {
    ui.settings_text(title, TextStyle::size(13.0).medium())
}

/// A note under a row: 11pt `ink2`.
pub(crate) fn caption(ui: &Ui, text: impl Into<String>) -> El {
    ui.settings_text(text, TextStyle::size(11.0).ink(Token::Ink2))
}

/// A folded part (고급, what remote workspaces can and cannot do): its heading opens it; whether it is open is kept under `key` while
/// Settings shows the tab (`open` the first time, e.g. open when something in it was already changed).
pub(crate) fn folded(ui: &mut Ui, key: &str, open: bool, title: String, body: El) -> El {
    let header = ui.settings_text(title, TextStyle::size(12.0).medium().ink(Token::Ink2));
    ui.expander(key, open, Content::El(Box::new(header)), Content::El(Box::new(body))).aid(key)
}

/// A category a deep link or a 0.5.3 save names, as it shows now: a saved 모바일 리모트 opens 원격 연결 on its 휴대폰 tab, a saved CLI
/// tab 구성 요소.
fn shown_category(id: &str) -> &str {
    let moved = sections::moved_category(id);
    if moved != id && moved == "remote" {
        remote::show_tab(remote::Tab::Phone);
    }
    moved
}

/// `SettingsCategoryGlyph`: the side list's Segoe Fluent symbol per category (the page draws its own Lucide icons for the known ones).
pub fn glyph(id: &str) -> &'static str {
    match id {
        "remote" => "\u{E968}",
        "models" => "\u{E950}",
        "styles" => "\u{E790}",
        "tools" => "\u{E7B8}",
        "about" => "\u{E946}",
        _ => "\u{E713}",
    }
}

/// 구성 요소' account statuses read again as it opens (C# `RefreshVisibleSettingsAccountsAsync`, the CLI tab's then), on a task of its
/// own so the accounts' read never runs under the surface's lock; never in a smoke.
fn refresh_accounts(host: &Arc<Host>, category: &str) {
    if category != "tools" || host.smoke.is_some() {
        return;
    }
    let host = host.clone();
    tauri::async_runtime::spawn(async move { crate::accounts::refresh(&host) });
}

/// The settings surface's model.
struct SettingsSurface {
    host: Arc<Host>,
    category: String,
    /// The window's dialog, held while Settings is open.
    _sheet: Sheet,
    /// Told when Settings closes (`openSettings` answers then).
    closed: Option<tokio::sync::oneshot::Sender<()>>,
}

impl Drop for SettingsSurface {
    fn drop(&mut self) {
        if let Some(closed) = self.closed.take() {
            let _ = closed.send(());
        }
    }
}

impl SurfaceModel for SettingsSurface {
    fn view(&mut self, ui: &mut Ui) -> View {
        let locale = ui.locale();
        let nav = available(&locale)
            .into_iter()
            .map(|c| NavItem { id: c.id.to_string(), title: c.title, glyph: glyph(c.id).to_string() })
            .collect();
        let sections = groups(&locale, &self.category)
            .into_iter()
            .map(|group| {
                let body = (group.build)(&self.host, ui);
                Section { title: Some(group.title), beta: group.beta, node: Some(ui.section_body(body)) }
            })
            .collect();
        View::Settings {
            title: locale.get("settings.title"),
            close: locale.get("settings.closeButton"),
            nav,
            selected: self.category.clone(),
            sections,
            width: metrics::SETTINGS_WIDTH,
            height: metrics::SETTINGS_HEIGHT,
        }
    }

    /// Any button closes Settings.
    fn button(&mut self, _which: Option<&str>) -> Next {
        Next::Close
    }

    /// `surface.Navigate`: nothing for the category shown or one not available; else that category's boxes, and `SettingsPane`
    /// saved (C#'s `Act(UpdateAsync)`: the banner cleared first, a failure shown there).
    fn navigate(&mut self, to: &str) -> bool {
        let moved = sections::moved_category(to) != to;
        let to = shown_category(to);
        if !available(&self.host.locale).iter().any(|c| c.id == to) {
            return false;
        }
        if to == self.category {
            // A 모바일 리모트 link while 원격 연결 shows: only its top tab turned.
            return moved;
        }
        self.category = to.to_string();
        refresh_accounts(&self.host, to);
        if to == "models" {
            // omc's and Ouroboros's settings read again off the surface's lock (C# `LoadTools` on every build).
            models::reload_tools(&self.host);
        }
        let mut view = self.host.view();
        view.error = None;
        if let Err(error) = view.update(|state| state.settings_pane = Some(to.to_string())) {
            view.error = Some(error);
        }
        true
    }
}

/// `ShowWebSettingsAsync` up to its wait: none when Settings is open already or another sheet holds the window.
fn start(host: &Arc<Host>) -> Option<tokio::sync::oneshot::Receiver<()>> {
    if crate::native::is_open(host, SURFACE) {
        return None;
    }
    let sheet = Sheet::begin(host)?;
    // The text box's font lists, read once per run before any builder runs (C# `InstalledFamilies`).
    general::read_families(host);
    // The external editors 일반 › 에이전트 창's 외부 편집기 lists, looked for again in the background (editor.rs).
    crate::editor::refresh_in_background(host);
    // omc's and Ouroboros's settings for the phase models box (C# `LoadTools`; the fixture in a smoke).
    models::read_tools(host);
    let saved = host.view().snapshot.settings_pane.clone().map(|pane| shown_category(&pane).to_string());
    let categories = available(&host.locale);
    let category = saved.filter(|pane| categories.iter().any(|c| c.id == pane)).unwrap_or_else(|| categories[0].id.to_string());
    refresh_accounts(host, &category);
    let (closed, done) = tokio::sync::oneshot::channel();
    crate::native::open(
        host,
        SURFACE,
        Place::Surface,
        Box::new(SettingsSurface { host: host.clone(), category, _sheet: sheet, closed: Some(closed) }),
    );
    Some(done)
}

/// `settings::open`: Settings opened (when it can be) and, when `category` is given, brought to that category (C# `OpenSettings`
/// then `SelectSettingsCategory`).
#[allow(dead_code, reason = "a seam W6-A1 (the slash palette) and W6-C calls")]
pub fn open(host: &Arc<Host>, category: Option<&str>) {
    let _ = start(host);
    if let Some(category) = category {
        select(host, category);
    }
}

/// `SelectWebSettingsCategory`: true when Settings is open (and now on `id`).
pub fn select(host: &Arc<Host>, id: &str) -> bool {
    if !crate::native::is_open(host, SURFACE) {
        return false;
    }
    if crate::native::navigate(host, SURFACE, id) {
        crate::native::render(host, SURFACE);
    }
    true
}

/// `CloseWebSettings`.
#[allow(dead_code, reason = "a seam W6-A1 and W6-D calls")]
pub fn close(host: &Arc<Host>) {
    crate::native::close(host, SURFACE);
}

/// C# `OpenHelp` (the sidebar's help symbol, F1, Settings › 앱 정보): the guide's folder in the language the app shows, in the user's
/// browser. A smoke opens nothing (the browser would outlive it): the address is logged instead.
pub async fn open_help(host: &Host) -> Result<(), String> {
    let address = help_site::current(&host.locale);
    if host.smoke.is_some() {
        host.log.line(&format!("smoke: help not opened: {address}"));
        return Ok(());
    }
    tauri::async_runtime::spawn_blocking(move || crate::platform::open_url(&address)).await.map_err(|error| error.to_string())?
}

pub fn register(methods: &mut Registry) {
    // C# `openSettings`: `if (!dialogOpen) await OpenSettings()`, which answers once Settings closed.
    methods.add("openSettings", |host: Arc<Host>, _| async move {
        if host.view().dialog_open {
            return Ok(Value::Null);
        }
        crate::frame::act(&host, async {
            if let Some(closed) = start(&host) {
                let _ = closed.await;
            }
            Ok(())
        })
        .await
    });
    methods.add("openHelp", |host: Arc<Host>, _| async move {
        open_help(&host).await?;
        Ok(Value::Null)
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every available category has its boxes, each box a heading, and the slots' headings are SettingsSections' (C#
    /// `SettingsGroups`).
    #[test]
    fn every_category_has_its_boxes_in_the_macs_order() {
        let locale = Locale::new("ko");
        let titles = |category: &str| groups(&locale, category).into_iter().map(|g| g.title).collect::<Vec<_>>();
        for category in available(&locale) {
            let boxes = groups(&locale, category.id);
            assert!(!boxes.is_empty() && boxes.iter().all(|g| !g.title.is_empty()), "{}", category.id);
        }
        assert_eq!(
            titles("general"),
            vec![
                "화면".to_string(),
                locale.get("settings.text.sectionTitle"),
                locale.get("settings.agents.sectionTitle"),
                locale.get("daemon.settings.sectionTitle")
            ]
        );
        assert_eq!(titles("tools"), vec![locale.get("settings.components.sectionTitle"), locale.get("settings.toolkit.sectionTitle")]);
        assert!(groups(&locale, "cli").is_empty(), "the CLI tab's accounts and updates are 구성 요소' rows");
        assert_eq!(shown_category("cli"), "tools", "a saved CLI tab opens 구성 요소");
        assert_eq!(titles("remote"), vec!["원격 연결".to_string()]);
        assert_eq!(titles("about"), vec![locale.get("settings.appUpdate.sectionTitle"), "앱 정보".to_string()]);
        assert!(groups(&locale, "mobile").is_empty(), "모바일 리모트 is 원격 연결's 휴대폰 tab");
        assert!(groups(&locale, "elsewhere").is_empty());
        assert!(available(&locale).iter().flat_map(|c| groups(&locale, c.id)).all(|g| !g.beta), "no box is a paused beta");
    }

    #[test]
    fn the_glyphs_are_segoe_fluents_code_points() {
        assert_eq!(glyph("general"), "\u{E713}");
        assert_eq!(glyph("companion"), "\u{E713}", "the paw is a path in WinUI; the list's glyph falls back to the gear");
        assert_eq!(glyph("remote"), "\u{E968}");
    }
}
