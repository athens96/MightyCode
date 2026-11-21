//! M-F: the Mac's menu bar through Tauri's Menu API (Swift `MightyCodeApp.swift:62-134`, refined to macOS HIG):
//! - Application menu (`Mighty Code`): About (with version metadata), Check for Updates…, Settings… (⌘,), Services, Hide/Show, Quit.
//! - File: Open Folder… (⌘O), New Folder…, Create from Git…, separator, New Claude/Codex/Antigravity Pane (⌘N), Add Terminal Pane (⌘T),
//!   separator, Close Window (⌘W).
//! - Edit: Undo/Redo, Cut/Copy/Paste/Select All (enables ⌘C/⌘V/⌘X/⌘A/⌘Z in WKWebView text fields and native terminal views).
//! - View: Toggle Sidebar (⌃⌘S, live Collapse/Expand label), Work Status (dashboard), Layout presets (Grid, Side by side, Focus, Tabs),
//!   Text size (⌘= ⌘- ⌘0), Enter Full Screen.
//! - Workspace: Search Workspaces (⌘K), separator, Open Files Pane (⇧⌘E), Conversation Search (⇧⌘F), Review Queue (⇧⌘R), Work Map,
//!   Usage Report, separator, Install 'mighty' shell command, separator, Input Method recovery & diagnostics.
//! - Window and Help (⌘?, handled on the host so Help works even while Settings or a sheet is open).
//!
//! Windows has no menu bar.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use mighty_core::locale::Locale;
use serde_json::json;
use tauri::Manager;
use tauri::menu::{AboutMetadata, Menu, MenuItem, MenuItemBuilder, PredefinedMenuItem, Submenu, SubmenuBuilder};

use crate::host::Host;

/// Who runs an item.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Act {
    /// The page, through the `menu` push (its id is the action).
    Page,
    /// The host.
    Host,
}

/// One of the app's own items: its id (the page's action), its words' locale key, its accelerator (Tauri's syntax) and who runs it.
/// An id of `-` is a separator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Item {
    pub id: &'static str,
    pub key: &'static str,
    pub accelerator: Option<&'static str>,
    pub act: Act,
}

const fn page(id: &'static str, key: &'static str, accelerator: Option<&'static str>) -> Item {
    Item { id, key, accelerator, act: Act::Page }
}

const fn host(id: &'static str, key: &'static str) -> Item {
    Item { id, key, accelerator: None, act: Act::Host }
}

const fn host_key(id: &'static str, key: &'static str, accelerator: Option<&'static str>) -> Item {
    Item { id, key, accelerator, act: Act::Host }
}

const SEPARATOR: Item = Item { id: "-", key: "", accelerator: None, act: Act::Host };

/// Application menu (`Mighty Code`) own items placed between About and Services (macOS HIG standard location for Check for Updates… and Settings… ⌘,).
pub const APP: [Item; 3] = [host("checkUpdates", "menu.checkForUpdates"), SEPARATOR, host_key("settings", "menu.settings", Some("Cmd+,"))];

/// File (Swift's `CommandGroup(replacing: .newItem)` + pane creation group).
pub const FILE: [Item; 8] = [
    page("openFolder", "menu.openProject", Some("Cmd+O")),
    page("newFolder", "workspace.add.newFolder", None),
    page("fromGit", "workspace.add.fromGit", None),
    SEPARATOR,
    page("newPane", "menu.newClaudePane", Some("Cmd+N")),
    page("newCodexPane", "menu.newCodexPane", None),
    page("newAntigravityPane", "menu.newAntigravityPane", None),
    page("newShell", "menu.addTerminalPane", Some("Cmd+T")),
];

/// View (sidebar, dashboard, layout presets, and text size).
pub const VIEW: [Item; 11] = [
    page("toggleSidebar", "sidebar.collapse", Some("Ctrl+Cmd+S")),
    page("dashboard", "phone.dashboard.title", None),
    SEPARATOR,
    page("layoutGrid", "layout.mode.grid", None),
    page("layoutColumns", "layout.mode.columns", None),
    page("layoutFocus", "layout.mode.focus", None),
    page("layoutTabs", "layout.mode.tabs", None),
    SEPARATOR,
    page("textLarger", "menu.textBigger", Some("Cmd+=")),
    page("textSmaller", "menu.textSmaller", Some("Cmd+-")),
    page("textReset", "menu.textActualSize", Some("Cmd+0")),
];

/// Workspace (Swift's `CommandMenu(L("menu.workspace"))`, grouped by navigation, tools/panels, CLI setup, and IME diagnostics).
pub const WORKSPACE: [Item; 12] = [
    page("search", "menu.searchWorkspaces", Some("Cmd+K")),
    SEPARATOR,
    page("files", "menu.showFiles", Some("Shift+Cmd+E")),
    page("conversationSearch", "conversationSearch.open", Some("Shift+Cmd+F")),
    page("reviewQueue", "review.open", Some("Shift+Cmd+R")),
    page("workMap", "workMap.open", None),
    page("usage", "usage.open", None),
    SEPARATOR,
    host("shellCommand", "shellCommand.install"),
    SEPARATOR,
    host("reconnectInputMethod", "menu.reconnectInputMethod"),
    host("saveInputDiagnostics", "menu.saveInputDiagnostics"),
];

/// Help (Swift's `CommandGroup(replacing: .help)`, the standard ⌘?: the `?` character, so it works on any layout; run on Host so it
/// opens even when Settings or a dialog sheet is up).
pub const HELP: [Item; 1] = [host_key("help", "menu.help", Some("Cmd+?"))];

/// Every item of the app's own.
pub fn items() -> impl Iterator<Item = &'static Item> {
    APP.iter().chain(FILE.iter()).chain(VIEW.iter()).chain(WORKSPACE.iter()).chain(HELP.iter()).filter(|item| item.id != "-")
}

/// Resolves the localized label of an item, keeping literal `locale.get("…")` references visible to `scripts/check-locales.js`.
pub fn label(locale: &Locale, key: &str) -> String {
    match key {
        "menu.checkForUpdates" => locale.get("menu.checkForUpdates"),
        "menu.settings" => locale.get("menu.settings"),
        "menu.openProject" => locale.get("menu.openProject"),
        "workspace.add.newFolder" => locale.get("workspace.add.newFolder"),
        "workspace.add.fromGit" => locale.get("workspace.add.fromGit"),
        "menu.newClaudePane" => locale.get("menu.newClaudePane"),
        "menu.newCodexPane" => locale.get("menu.newCodexPane"),
        "menu.newAntigravityPane" => locale.get("menu.newAntigravityPane"),
        "menu.addTerminalPane" => locale.get("menu.addTerminalPane"),
        "sidebar.collapse" => locale.get("sidebar.collapse"),
        "sidebar.expand" => locale.get("sidebar.expand"),
        "phone.dashboard.title" => locale.get("phone.dashboard.title"),
        "layout.mode.grid" => locale.get("layout.mode.grid"),
        "layout.mode.columns" => locale.get("layout.mode.columns"),
        "layout.mode.focus" => locale.get("layout.mode.focus"),
        "layout.mode.tabs" => locale.get("layout.mode.tabs"),
        "menu.textBigger" => locale.get("menu.textBigger"),
        "menu.textSmaller" => locale.get("menu.textSmaller"),
        "menu.textActualSize" => locale.get("menu.textActualSize"),
        "menu.searchWorkspaces" => locale.get("menu.searchWorkspaces"),
        "menu.showFiles" => locale.get("menu.showFiles"),
        "conversationSearch.open" => locale.get("conversationSearch.open"),
        "review.open" => locale.get("review.open"),
        "workMap.open" => locale.get("workMap.open"),
        "usage.open" => locale.get("usage.open"),
        "shellCommand.install" => locale.get("shellCommand.install"),
        "menu.reconnectInputMethod" => locale.get("menu.reconnectInputMethod"),
        "menu.saveInputDiagnostics" => locale.get("menu.saveInputDiagnostics"),
        "menu.help" => locale.get("menu.help"),
        other => locale.get(other),
    }
}

/// The current menu title of `item` for `host` (`toggleSidebar` flips between collapse and expand).
pub fn item_title(host: &Host, item: &Item) -> String {
    if item.id == "toggleSidebar" && host.view().snapshot.sidebar_collapsed {
        return label(&host.locale, "sidebar.expand");
    }
    label(&host.locale, item.key)
}

/// The standard menus' and items' words as AppKit says them in the app's languages.
#[derive(Debug, Clone, Copy)]
pub struct Standard {
    pub file: &'static str,
    pub edit: &'static str,
    pub view: &'static str,
    pub window: &'static str,
    pub help: &'static str,
    pub about: &'static str,
    pub services: &'static str,
    pub hide: &'static str,
    pub hide_others: &'static str,
    pub show_all: &'static str,
    pub quit: &'static str,
    pub close: &'static str,
    pub undo: &'static str,
    pub redo: &'static str,
    pub cut: &'static str,
    pub copy: &'static str,
    pub paste: &'static str,
    pub select_all: &'static str,
    pub full_screen: &'static str,
    pub minimize: &'static str,
    pub zoom: &'static str,
    pub front: &'static str,
}

pub fn standard(language: &str) -> Standard {
    match language {
        "ko" => Standard {
            file: "파일",
            edit: "편집",
            view: "보기",
            window: "윈도우",
            help: "도움말",
            about: "Mighty Code에 관하여",
            services: "서비스",
            hide: "Mighty Code 가리기",
            hide_others: "기타 가리기",
            show_all: "모두 보기",
            quit: "Mighty Code 종료",
            close: "닫기",
            undo: "실행 취소",
            redo: "실행 복귀",
            cut: "오려두기",
            copy: "복사하기",
            paste: "붙여넣기",
            select_all: "전체 선택",
            full_screen: "전체 화면 시작",
            minimize: "최소화",
            zoom: "확대/축소",
            front: "모두 앞으로 가져오기",
        },
        "ja" => Standard {
            file: "ファイル",
            edit: "編集",
            view: "表示",
            window: "ウインドウ",
            help: "ヘルプ",
            about: "Mighty Codeについて",
            services: "サービス",
            hide: "Mighty Codeを非表示",
            hide_others: "ほかを非表示",
            show_all: "すべてを表示",
            quit: "Mighty Codeを終了",
            close: "閉じる",
            undo: "取り消す",
            redo: "やり直す",
            cut: "カット",
            copy: "コピー",
            paste: "ペースト",
            select_all: "すべてを選択",
            full_screen: "フルスクリーンにする",
            minimize: "しまう",
            zoom: "拡大/縮小",
            front: "すべてを手前に移動",
        },
        "zh" => Standard {
            file: "文件",
            edit: "编辑",
            view: "显示",
            window: "窗口",
            help: "帮助",
            about: "关于 Mighty Code",
            services: "服务",
            hide: "隐藏 Mighty Code",
            hide_others: "隐藏其他",
            show_all: "全部显示",
            quit: "退出 Mighty Code",
            close: "关闭",
            undo: "撤销",
            redo: "重做",
            cut: "剪切",
            copy: "拷贝",
            paste: "粘贴",
            select_all: "全选",
            full_screen: "进入全屏幕",
            minimize: "最小化",
            zoom: "缩放",
            front: "前置全部窗口",
        },
        _ => Standard {
            file: "File",
            edit: "Edit",
            view: "View",
            window: "Window",
            help: "Help",
            about: "About Mighty Code",
            services: "Services",
            hide: "Hide Mighty Code",
            hide_others: "Hide Others",
            show_all: "Show All",
            quit: "Quit Mighty Code",
            close: "Close",
            undo: "Undo",
            redo: "Redo",
            cut: "Cut",
            copy: "Copy",
            paste: "Paste",
            select_all: "Select All",
            full_screen: "Enter Full Screen",
            minimize: "Minimize",
            zoom: "Zoom",
            front: "Bring All to Front",
        },
    }
}

/// What the smoke records instead of acting (each item's id as it is chosen).
pub type Recorder = Arc<dyn Fn(&str) + Send + Sync>;

static RECORDER: Mutex<Option<Recorder>> = Mutex::new(None);
static SIDEBAR_ITEM: Mutex<Option<MenuItem<tauri::Wry>>> = Mutex::new(None);

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// The smoke's recorder (smoke_companion.rs): while set, a chosen item is recorded and not run.
#[cfg(feature = "dev-harness")]
pub fn set_recorder(recorder: Option<Recorder>) {
    *lock(&RECORDER) = recorder;
}

/// Keeps the View › Toggle Sidebar menu label in sync with `sidebar_collapsed`.
pub fn sync_sidebar(host: &Host) {
    let Some(item) = lock(&SIDEBAR_ITEM).clone() else { return };
    let collapsed = host.view().snapshot.sidebar_collapsed;
    let _ = item.set_text(label(&host.locale, if collapsed { "sidebar.expand" } else { "sidebar.collapse" }));
}

/// Installs the menu bar (the Mac only): its events handled from now on, the menu made once the host (its language) is there.
pub fn install(app: &tauri::AppHandle) -> tauri::Result<()> {
    if !crate::platform::has_menu_bar() {
        return Ok(());
    }
    app.on_menu_event(|app, event| chosen(app, event.id().as_ref()));
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        // main.rs installs the menu before it manages the host.
        for _ in 0..200 {
            if let Some(host) = app.try_state::<Arc<Host>>().map(|h| h.inner().clone()) {
                match build(&app, &host).and_then(|menu| app.set_menu(menu)) {
                    Ok(_) => {
                        host.log.line("menu: installed");
                        for item in items() {
                            if let Some(key) = item.accelerator.and_then(character_key) {
                                crate::platform::set_menu_character_key(&app, &item_title(&host, item), key);
                            }
                        }
                    }
                    Err(error) => host.log.line(&format!("menu: {error}")),
                }
                return;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    });
    Ok(())
}

/// The character of a ⌘ key that names a character, not a key code (`Cmd+?`: the `?` wherever the layout puts it), which Tauri's
/// accelerators cannot take (review item 14).
pub fn character_key(accelerator: &str) -> Option<&str> {
    accelerator.strip_prefix("Cmd+").filter(|key| *key == "?")
}

fn build_item(app: &tauri::AppHandle, host: &Host, item: &Item) -> tauri::Result<MenuItem<tauri::Wry>> {
    let mut entry = MenuItemBuilder::with_id(item.id, item_title(host, item));
    // Tauri's accelerators name key codes; a character key (⌘?) is set on the item by the platform after the menu is in.
    if let Some(accelerator) = item.accelerator.filter(|a| character_key(a).is_none()) {
        entry = entry.accelerator(accelerator);
    }
    let built = entry.build(app)?;
    if item.id == "toggleSidebar" {
        *lock(&SIDEBAR_ITEM) = Some(built.clone());
    }
    Ok(built)
}

/// The menu bar in the app's language.
pub fn build(app: &tauri::AppHandle, host: &Host) -> tauri::Result<Menu<tauri::Wry>> {
    let locale = &host.locale;
    let words = standard(locale.language());
    let own = |title: &str, items: &[Item]| -> tauri::Result<Submenu<tauri::Wry>> {
        let mut built = SubmenuBuilder::new(app, title);
        for item in items {
            built = if item.id == "-" { built.separator() } else { built.item(&build_item(app, host, item)?) };
        }
        built.build()
    };
    let version = crate::settings::about::version_text(host);
    let about_meta =
        AboutMetadata { name: Some("Mighty Code".into()), version: (!version.is_empty()).then_some(version), ..Default::default() };
    let mut application =
        SubmenuBuilder::new(app, "Mighty Code").item(&PredefinedMenuItem::about(app, Some(words.about), Some(about_meta))?);
    for item in &APP {
        application = if item.id == "-" { application.separator() } else { application.item(&build_item(app, host, item)?) };
    }
    let application = application
        .separator()
        .item(&PredefinedMenuItem::services(app, Some(words.services))?)
        .separator()
        .item(&PredefinedMenuItem::hide(app, Some(words.hide))?)
        .item(&PredefinedMenuItem::hide_others(app, Some(words.hide_others))?)
        .item(&PredefinedMenuItem::show_all(app, Some(words.show_all))?)
        .separator()
        .item(&PredefinedMenuItem::quit(app, Some(words.quit))?)
        .build()?;
    let file = own(words.file, &FILE)?;
    file.append(&PredefinedMenuItem::separator(app)?)?;
    file.append(&PredefinedMenuItem::close_window(app, Some(words.close))?)?;
    let edit = SubmenuBuilder::new(app, words.edit)
        .item(&PredefinedMenuItem::undo(app, Some(words.undo))?)
        .item(&PredefinedMenuItem::redo(app, Some(words.redo))?)
        .separator()
        .item(&PredefinedMenuItem::cut(app, Some(words.cut))?)
        .item(&PredefinedMenuItem::copy(app, Some(words.copy))?)
        .item(&PredefinedMenuItem::paste(app, Some(words.paste))?)
        .item(&PredefinedMenuItem::select_all(app, Some(words.select_all))?)
        .build()?;
    let view = own(words.view, &VIEW)?;
    view.append(&PredefinedMenuItem::separator(app)?)?;
    view.append(&PredefinedMenuItem::fullscreen(app, Some(words.full_screen))?)?;
    let workspace = own(&locale.get("menu.workspace"), &WORKSPACE)?;
    let window = SubmenuBuilder::new(app, words.window)
        .item(&PredefinedMenuItem::minimize(app, Some(words.minimize))?)
        .item(&PredefinedMenuItem::maximize(app, Some(words.zoom))?)
        .separator()
        .item(&PredefinedMenuItem::bring_all_to_front(app, Some(words.front))?)
        .build()?;
    let help = own(words.help, &HELP)?;
    let menu = Menu::with_items(app, &[&application, &file, &edit, &view, &workspace, &window, &help])?;
    crate::platform::name_standard_menus(&window, &help)?;
    Ok(menu)
}

/// An item chosen (or its key equivalent pressed).
fn chosen(app: &tauri::AppHandle, id: &str) {
    let Some(item) = items().find(|item| item.id == id) else { return };
    let recorder = lock(&RECORDER).clone();
    if let Some(record) = recorder {
        record(item.id);
        return;
    }
    let Some(host) = app.try_state::<Arc<Host>>().map(|h| h.inner().clone()) else { return };
    match item.act {
        Act::Page => {
            let view = host.view();
            host.push(&view, "menu", json!({"kind": "menu", "action": item.id}).to_string());
        }
        Act::Host => {
            let id = item.id;
            tauri::async_runtime::spawn(async move { run(&host, id).await });
        }
    }
}

/// The host's own items.
async fn run(host: &Arc<Host>, id: &str) {
    match id {
        "settings" => crate::settings::open(host, None),
        "checkUpdates" => {
            crate::settings::open(host, Some("about"));
            if let Some(update) = crate::app_update::of(host) {
                let can_check =
                    update.core.coordinator.state().phase == mighty_core::app_update_view::Phase::Idle && update.core.press_allowed();
                if can_check {
                    crate::app_update::press(host);
                }
            }
        }
        "help" => {
            if let Err(error) = crate::settings::open_help(host).await {
                host.log.line(&format!("help: not opened: {error}"));
            }
        }
        "shellCommand" => crate::shell_command::install_from_menu(host).await,
        "reconnectInputMethod" => {
            let Some(page) = host.app.get_webview_window(crate::host::WINDOW) else { return };
            match crate::platform::reconnect_input_method(page.as_ref()).await {
                Ok(key) => notice(host, &host.locale.get(key)).await,
                Err(error) => host.log.line(&format!("input method: not reconnected: {error}")),
            }
        }
        "saveInputDiagnostics" => {
            let Some(page) = host.app.get_webview_window(crate::host::WINDOW) else { return };
            let Some(paths) = host.app.try_state::<Arc<dyn mighty_platform::Paths>>().map(|p| p.inner().clone()) else { return };
            match crate::platform::save_input_diagnostics(page.as_ref(), paths.data_dir()).await {
                // Swift shows the file in Finder.
                Ok(file) => {
                    if let Ok(Err(error)) = tauri::async_runtime::spawn_blocking(move || crate::platform::show_in_explorer(&file)).await {
                        host.log.line(&format!("input method: diagnostics saved, not shown: {error}"));
                    }
                }
                Err(error) => {
                    host.log.line(&format!("input method: diagnostics not saved: {error}"));
                    host.view().error = Some(host.locale.get("menu.saveInputDiagnosticsFailed"));
                }
            }
        }
        _ => {}
    }
}

/// A sheet in the page with `message` and a close button (Swift's alert).
async fn notice(host: &Arc<Host>, message: &str) {
    let Some(_sheet) = crate::frame::Sheet::begin(host) else { return };
    let (_, answer) = crate::dialogs::of(host).open(json!({
        "type": "confirm", "title": host.locale.get("menu.reconnectInputMethod"), "message": message, "close": host.locale.get("common.close"),
    }));
    let _ = answer.await;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every item's words are in every catalogue, Settings (⌘,) lives in the Application menu, each id is unique, and all ⌘ shortcuts match.
    #[test]
    fn the_menu_is_swifts() {
        let keys = mighty_core::locale::keys();
        let mut ids = std::collections::HashSet::new();
        for item in items() {
            assert!(keys.contains(item.key), "{} has no words", item.key);
            assert!(ids.insert(item.id), "{} twice", item.id);
        }
        assert!(APP.iter().any(|i| i.id == "settings" && i.accelerator == Some("Cmd+,")), "Settings (⌘,) is in the Application menu");
        assert!(APP.iter().any(|i| i.id == "checkUpdates"), "Check for Updates… is in the Application menu");
        assert!(!WORKSPACE.iter().any(|i| i.id == "settings"), "Settings is no longer buried in Workspace");
        let shortcuts: Vec<(&str, &str)> = items().filter_map(|i| i.accelerator.map(|a| (i.id, a))).collect();
        assert_eq!(
            shortcuts,
            [
                ("settings", "Cmd+,"),
                ("openFolder", "Cmd+O"),
                ("newPane", "Cmd+N"),
                ("newShell", "Cmd+T"),
                ("toggleSidebar", "Ctrl+Cmd+S"),
                ("textLarger", "Cmd+="),
                ("textSmaller", "Cmd+-"),
                ("textReset", "Cmd+0"),
                ("search", "Cmd+K"),
                ("files", "Shift+Cmd+E"),
                ("conversationSearch", "Shift+Cmd+F"),
                ("reviewQueue", "Shift+Cmd+R"),
                ("help", "Cmd+?"),
            ]
        );
        assert_eq!(items().filter(|i| i.act == Act::Host).count(), 6);
        assert_eq!(character_key("Cmd+?"), Some("?"));
        assert_eq!(character_key("Cmd+K"), None);
        for language in mighty_core::locale::LANGUAGES {
            assert!(!standard(language).edit.is_empty());
        }
        assert_eq!(Locale::new("ko").get("menu.workspace"), "워크스페이스");
    }
}
