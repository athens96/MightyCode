//! The forms (mighty-tools `view`) read as the page reads them: the setup screen over the smoke's fixed probes (MainWindow.SetupSmoke.cs's
//! rows, ticks, buttons and plan, as the page now sees them), its controls reaching the host's actions through the act table, the
//! confirmation D10, Settings › 구성 요소 (a row per item, the CLI tab's accounts and updates among them), the toolkit box with its file error,
//! rows and results, and D7. Each is drawn by mighty-native's surfaces exactly as the host pushes it.

mod common;

use std::sync::{Arc, Mutex};

use mighty_accounts::accounts::{CliAccountStatus, CliLoginOption};
use mighty_accounts::view::{AccountActions, AccountRow, AccountsModel};
use mighty_core::locale::Locale;
use mighty_core::text_appearance::TextAppearance;
use mighty_native::surface::{DialogSurface, SurfaceModel, Surfaces};
use mighty_native::tokens::Theme;
use mighty_native::ui::{Env, Ui};
use mighty_native::view::{Place, View};
use mighty_run::strings;
use mighty_tools::setup::screen::{self, SetupScreen};
use mighty_tools::toolkit::file::{ToolkitEntrySource, ToolkitFileEntry, ToolkitInstallSpec};
use mighty_tools::toolkit::runner::{PlanAction, ToolkitPlanItem, ToolkitRunItem, Verdict, presentation};
use mighty_tools::view::components::{self, ComponentActions, ComponentsActions, ComponentsModel, ComponentsView};
use mighty_tools::view::{self, SetupActions, SetupExtras, ToolkitActions, ToolkitModel, ToolkitRow};
use mighty_updates::cli_update::CliUpdateResult;
use mighty_updates::plugins::PluginAutoUpdateResult;
use mighty_updates::view::{CliUpdateActions, CliUpdateBox};
use serde_json::Value;

fn env() -> Env {
    Env { theme: Theme::Dark, text: TextAppearance::default(), locale: Locale::new("ko") }
}

/// What the controls asked the host to do.
#[derive(Default)]
struct Recorded(Mutex<Vec<String>>);

impl Recorded {
    fn said(&self, what: String) {
        self.0.lock().unwrap().push(what);
    }
    fn all(&self) -> Vec<String> {
        self.0.lock().unwrap().clone()
    }
}

impl SetupActions for Recorded {
    fn toggle(&self, id: &str) {
        self.said(format!("toggle {id}"));
    }
    fn sign_in(&self, provider: &str) {
        self.said(format!("sign in {provider}"));
    }
    fn update(&self) {
        self.said("update".into());
    }
    fn open_page(&self, url: &str) {
        self.said(format!("page {url}"));
    }
    fn recheck(&self) {
        self.said("recheck".into());
    }
    fn install(&self) {
        self.said("install".into());
    }
    fn abandon(&self) {
        self.said("abandon".into());
    }
}

impl ToolkitActions for Recorded {
    fn add(&self) {
        self.said("add".into());
    }
    fn export(&self) {
        self.said("export".into());
    }
    fn import(&self) {
        self.said("import".into());
    }
    fn install(&self) {
        self.said("toolkit install".into());
    }
    fn approve(&self, id: &str) {
        self.said(format!("approve {id}"));
    }
    fn remove(&self, id: &str) {
        self.said(format!("remove {id}"));
    }
}

/// A surface that builds the setup screen from a screen held by the check.
struct SetupModel {
    screen: Arc<Mutex<SetupScreen>>,
    extras: SetupExtras,
    actions: Arc<Recorded>,
    never_show: Arc<Mutex<bool>>,
}

impl SurfaceModel for SetupModel {
    fn view(&mut self, ui: &mut Ui) -> View {
        let list = view::setup_content(ui, &self.screen.lock().unwrap(), &self.extras, self.actions.clone());
        let never = self.never_show.clone();
        view::setup_view(list, *self.never_show.lock().unwrap(), move |on| *never.lock().unwrap() = on)
    }
}

/// A surface that builds a settings box (toned as the settings shell tones a box).
struct BoxModel(Box<dyn FnMut(&mut Ui) -> mighty_native::ui::El + Send>);

impl SurfaceModel for BoxModel {
    fn view(&mut self, ui: &mut Ui) -> View {
        let body = (self.0)(ui);
        View::Companion { title: "box".into(), body: Some(ui.section_body(body)) }
    }
}

/// Every node of a pushed view, depth first.
fn nodes(value: &Value, into: &mut Vec<Value>) {
    match value {
        Value::Object(object) => {
            if object.contains_key("t") {
                into.push(value.clone());
            }
            for child in object.values() {
                nodes(child, into);
            }
        }
        Value::Array(items) => items.iter().for_each(|item| nodes(item, into)),
        _ => {}
    }
}

fn pushed(surfaces: &Surfaces, id: &str) -> Value {
    surfaces.forget_sent();
    serde_json::from_str(&surfaces.render(id, &env()).expect("a push")).unwrap()
}

fn by_aid(view: &Value, aid: &str) -> Option<Value> {
    let mut all = vec![];
    nodes(view, &mut all);
    all.into_iter().find(|node| node.get("aid").and_then(Value::as_str) == Some(aid))
}

fn control_id(node: &Value) -> i64 {
    node["id"].as_i64().unwrap_or_else(|| panic!("a control: {node}"))
}

fn smoke_screen() -> Arc<Mutex<SetupScreen>> {
    let mut screen = SetupScreen::new();
    let catalog = screen.load_catalog().unwrap();
    screen.finish_refresh(screen::smoke_fixed_results(&catalog), None, false, std::time::Instant::now());
    screen.open(Some("agy"));
    screen.preselect_once();
    screen.toggle("gh");
    Arc::new(Mutex::new(screen))
}

/// The setup screen the smoke opens (MainWindow.SetupSmoke.cs): its rows by automation id with Git, the focused Antigravity and
/// the picked gh ticked and glab not, Claude below its minimum offering its update and Tailscale its page, the install button on
/// with a plan, the Mac's tools absent, and the panel's own keys; each control reaches the host's actions.
#[test]
fn the_setup_screen_draws_the_smokes_rows_and_its_controls_reach_the_host() {
    common::korean();
    let actions = Arc::new(Recorded::default());
    let screen = smoke_screen();
    let never_show = Arc::new(Mutex::new(false));
    let surfaces = Surfaces::new();
    let model =
        SetupModel { screen: screen.clone(), extras: SetupExtras::default(), actions: actions.clone(), never_show: never_show.clone() };
    surfaces.open(view::SETUP_SURFACE, Place::Surface, Box::new(model));
    let page = pushed(&surfaces, view::SETUP_SURFACE);
    assert_eq!(page["kind"], "native");
    assert_eq!(page["place"], "surface");
    let v = &page["view"];
    assert_eq!(v["kind"], "setup");
    assert_eq!(v["title"], strings::get("setup.title"));
    assert_eq!(v["subtitle"], strings::get("setup.windows.subtitle"));
    assert_eq!(v["close"], strings::get("setup.button.later"));
    assert_eq!(v["chrome"], serde_json::json!({"kind": "panel", "panel": "setup", "icon": "wrench"}));
    assert!(v["width"] == 620 && v["height"] == 700);
    assert_eq!(v["sections"][0].get("title"), None, "the setup screen's one section has no heading");
    assert_eq!(v["footer"]["t"], "check");
    assert_eq!(v["footer"]["aid"], "setup-never-show");
    assert_eq!(v["footer"]["label"], strings::get("setup.button.neverShow"));
    for id in ["claude", "codex", "agy", "git", "node", "gh", "tailscale", "glab"] {
        assert!(by_aid(v, &format!("setup-row-{id}")).is_some(), "the {id} row");
    }
    for id in ["homebrew", "gstack", "bun"] {
        assert!(by_aid(v, &format!("setup-row-{id}")).is_none(), "the Mac's {id} is not listed");
    }
    let ticked = |id: &str| by_aid(v, &format!("setup-check-{id}")).map(|check| check["on"] == true);
    assert_eq!((ticked("git"), ticked("agy"), ticked("gh"), ticked("glab")), (Some(true), Some(true), Some(true), Some(false)));
    assert_eq!(ticked("codex"), None, "an installed CLI's box is hidden");
    assert_eq!(ticked("node"), Some(false), "a missing Node.js nothing picked needs: its box shows, unticked");
    assert_eq!(ticked("tailscale"), None, "a page is no install: its box is hidden");
    assert!(by_aid(v, "setup-update-claude").is_some() && by_aid(v, "setup-page-tailscale").is_some(), "the update and the page");
    let install = by_aid(v, "setup-install").expect("the install button");
    assert!(install.get("disabled").is_none() && install["accent"] == true, "on with a plan: {install}");
    assert_eq!(by_aid(v, "setup-recheck").unwrap()["label"], strings::get("setup.button.recheck"));
    // Each control's input reaches the host.
    for aid in ["setup-check-glab", "setup-update-claude", "setup-page-tailscale", "setup-recheck", "setup-install"] {
        let node = by_aid(v, aid).unwrap();
        let op = if node["t"] == "check" { "toggle" } else { "invoke" };
        assert!(surfaces.act(view::SETUP_SURFACE, control_id(&node), op, &Value::Null).run(|_| {}), "{aid} takes its input");
    }
    assert_eq!(actions.all(), ["toggle glab", "update", "page https://tailscale.com/download/windows", "recheck", "install"]);
    let never = by_aid(v, "setup-never-show").unwrap();
    surfaces.act(view::SETUP_SURFACE, control_id(&never), "toggle", &Value::Null).run(|_| {});
    assert!(*never_show.lock().unwrap(), "never-show ticks");
    // A running install: the boxes wait, the hint and the ring show; an empty plan turns the install off.
    {
        let mut held = screen.lock().unwrap();
        held.selection.clear();
        held.run = Some(screen::SetupRun {
            items: vec!["gh".into()],
            started: std::env::temp_dir().join("s"),
            done: std::env::temp_dir().join("d"),
            opened: std::time::Instant::now(),
            probed: std::time::Instant::now(),
            finished: false,
            not_started: true,
        });
        held.message = Some("why".into());
    }
    let running = pushed(&surfaces, view::SETUP_SURFACE);
    let v = &running["view"];
    assert_eq!(by_aid(v, "setup-install").unwrap()["disabled"], true);
    assert!(by_aid(v, "setup-run-end").is_some() && by_aid(v, "setup-message").is_some() && by_aid(v, "setup-run-not-started").is_some());
    let mut all = vec![];
    nodes(v, &mut all);
    assert!(all.iter().any(|n| n["t"] == "spinner"), "the ring turns while the install runs");
}

/// The sign-in a signed-out agent CLI offers, and the update that waits while a CLI update runs.
#[test]
fn the_setup_rows_offer_the_sign_in_and_a_waiting_update() {
    common::korean();
    let actions = Arc::new(Recorded::default());
    let extras = SetupExtras { signed_out: ["codex".to_string()].into(), cli_update_running: true, plugin_operations_running: false };
    let surfaces = Surfaces::new();
    let model = SetupModel { screen: smoke_screen(), extras, actions: actions.clone(), never_show: Arc::default() };
    surfaces.open(view::SETUP_SURFACE, Place::Surface, Box::new(model));
    let page = pushed(&surfaces, view::SETUP_SURFACE);
    let v = &page["view"];
    let sign_in = by_aid(v, "setup-signin-codex").expect("a signed-out CLI offers its sign-in");
    assert_eq!(sign_in["label"], strings::get("setup.button.signIn"));
    surfaces.act(view::SETUP_SURFACE, control_id(&sign_in), "invoke", &Value::Null).run(|_| {});
    let update = by_aid(v, "setup-update-claude").unwrap();
    assert!(update["label"] == strings::get("settings.components.inProgressLabel") && update["disabled"] == true, "{update}");
    assert_eq!(actions.all(), ["sign in codex"]);
}

/// D10: the confirmation's title, its preamble and steps by automation id, Run and Cancel with Cancel the default.
#[test]
fn the_setup_confirmation_lists_the_first_lines_and_every_step() {
    common::korean();
    let screen = smoke_screen();
    let confirmation = screen.lock().unwrap().confirmation(&Default::default()).unwrap();
    let surfaces = Surfaces::new();
    surfaces.open("dialog-1", Place::Dialog, Box::new(DialogSurface::new(view::setup_confirmation(&confirmation), |_| {})));
    let page = pushed(&surfaces, "dialog-1");
    let v = &page["view"];
    assert_eq!(v["title"], strings::get("setup.confirm.title"));
    assert_eq!(v["chrome"]["kind"], "sheet");
    let preamble = by_aid(v, "setup-confirm-preamble").expect("the first lines");
    assert!(preamble.to_string().contains("Set-Location -LiteralPath $HOME"));
    for id in ["agy", "git", "gh"] {
        assert!(by_aid(v, &format!("setup-confirm-step-{id}")).is_some(), "the {id} step");
    }
    assert_eq!(
        v["buttons"],
        serde_json::json!([{"id": "close", "label": strings::get("setup.confirm.cancel"), "disabled": false, "accent": true}, {"id": "primary", "label": strings::get("setup.confirm.run"), "disabled": false, "accent": false}])
    );
    assert!(page.to_string().contains("\"maxH\":340"), "the steps scroll past 340");
}

/// What 구성 요소' account and update controls asked of the host.
impl AccountActions for Recorded {
    fn change(&self, provider: &str) {
        self.said(format!("change {provider}"));
    }
    fn logout(&self, provider: &str) {
        self.said(format!("logout {provider}"));
    }
    fn sign_in(&self, provider: &str, _: CliLoginOption) {
        self.said(format!("sign in {provider}"));
    }
    fn cancel_wait(&self, provider: &str) {
        self.said(format!("cancel {provider}"));
    }
    fn reset_models(&self) {
        self.said("reset models".into());
    }
    fn reset_bedrock(&self) {
        self.said("reset bedrock".into());
    }
    fn refresh(&self, provider: &str) {
        self.said(format!("refresh {provider}"));
    }
    fn auto_login(&self, on: bool) {
        self.said(format!("auto login {on}"));
    }
}

impl ComponentActions for Recorded {
    fn install(&self, id: &str) {
        self.said(format!("install {id}"));
    }
    fn update(&self) {
        self.said("update".into());
    }
    fn open_page(&self, url: &str) {
        self.said(format!("page {url}"));
    }
    fn recheck(&self) {
        self.said("recheck".into());
    }
    fn abandon(&self) {
        self.said("abandon".into());
    }
    fn toggle(&self, id: &str) {
        self.said(format!("toggle {id}"));
    }
    fn show_problems(&self, only: bool) {
        self.said(format!("problems {only}"));
    }
}

/// The accounts as 구성 요소 reads them: Claude signed in with an account, Codex signed out, the rest still being read.
fn accounts_model() -> AccountsModel {
    let mut statuses = AccountsModel::from_statuses(&Default::default());
    statuses[0] = CliAccountStatus {
        logged_in: Some(true),
        account: Some("me@example.com".into()),
        plan: Some("Max".into()),
        ..CliAccountStatus::new("claude")
    };
    statuses[1] = CliAccountStatus { logged_in: Some(false), ..CliAccountStatus::new("codex") };
    let rows = statuses.into_iter().map(|status| AccountRow { status, pending: false, message: None, working: false }).collect();
    AccountsModel { rows, reading: false, auto_login: true }
}

/// The CLI updates' fixture (the settings smoke's): Claude updated and failed, Codex current, Antigravity skipped, Claude's plugins
/// updated.
fn updates_model() -> CliUpdateBox {
    CliUpdateBox {
        last_run: Some("15:04".into()),
        results: vec![
            CliUpdateResult::new("claude", "updated").with_versions(Some("2.1.270"), Some("2.1.271")),
            CliUpdateResult::new("codex", "current"),
            CliUpdateResult::new("gemini", "skipped"),
            CliUpdateResult::new("claude", "failed"),
        ],
        plugin_results: vec![("claude".into(), PluginAutoUpdateResult::new("succeeded", "detail"))],
        ..CliUpdateBox::default()
    }
}

/// 구성 요소 over `screen` with `view` (the rows opened, the filter), its every control's action recorded (the updates' start says
/// "start").
fn components_surface(screen: Arc<Mutex<SetupScreen>>, updates: CliUpdateBox, actions: Arc<Recorded>, view: ComponentsView) -> Surfaces {
    let accounts = accounts_model();
    let start = actions.clone();
    let model = BoxModel(Box::new(move |ui| {
        let all = ComponentsActions {
            components: actions.clone(),
            accounts: actions.clone(),
            updates: CliUpdateActions {
                start: Arc::new({
                    let start = start.clone();
                    move || start.said("start".into())
                }),
                ..CliUpdateActions::none()
            },
        };
        let screen = screen.lock().unwrap();
        components::components_tab(ui, &ComponentsModel { screen: &screen, accounts: &accounts, updates: &updates, view: &view }, &all)
    }));
    let surfaces = Surfaces::new();
    surfaces.open("box", Place::Popover, Box::new(model));
    surfaces
}

/// 구성 요소 built over the smoke's probes with `view` (no surface): what a check reads off the built tree.
fn built_components(view: ComponentsView) -> mighty_native::ui::El {
    let actions = Arc::new(Recorded::default());
    let (env, mut locals) = (env(), mighty_native::ui::Locals::default());
    let screen = smoke_screen();
    let (accounts, updates) = (accounts_model(), updates_model());
    let all = ComponentsActions { components: actions.clone(), accounts: actions, updates: CliUpdateActions::none() };
    let screen = screen.lock().unwrap();
    components::components_tab(
        &mut Ui::new(&env, &mut locals),
        &ComponentsModel { screen: &screen, accounts: &accounts, updates: &updates, view: &view },
        &all,
    )
}

fn opened(ids: &[&str]) -> ComponentsView {
    ComponentsView { open: ids.iter().map(|id| id.to_string()).collect(), ..ComponentsView::default() }
}

fn text_of(view: &Value, aid: &str) -> String {
    let node = by_aid(view, aid).unwrap_or_else(|| panic!("no {aid}"));
    let mut all = vec![];
    nodes(&node, &mut all);
    all.into_iter().find_map(|n| n["text"].as_str().map(str::to_string)).unwrap_or_else(|| panic!("no text {aid}"))
}

fn press(surfaces: &Surfaces, view: &Value, aid: &str) {
    let node = by_aid(view, aid).unwrap_or_else(|| panic!("no {aid}"));
    let control = if node.get("id").is_some() { node } else { first_control(&node).unwrap_or_else(|| panic!("no control in {aid}")) };
    surfaces.act("box", control_id(&control), "invoke", &Value::Null).run(|_| {});
}

fn first_control(node: &Value) -> Option<Value> {
    let mut all = vec![];
    nodes(node, &mut all);
    all.into_iter().find(|n| n.get("id").is_some())
}

/// The automation ids of the nodes under `aid` that are controls, in order.
fn controls_in(view: &Value, aid: &str) -> Vec<String> {
    let node = by_aid(view, aid).unwrap_or_else(|| panic!("no {aid}"));
    let mut all = vec![];
    nodes(&node, &mut all);
    all.into_iter().filter(|n| n.get("id").is_some()).filter_map(|n| n["aid"].as_str().map(str::to_string)).collect()
}

/// Settings › 구성 요소 over the smoke's probes (Claude out of date, Codex there, the rest missing; Windows' items), every row
/// closed: the header (the filter, the refresh, 모두 업데이트), the three groups' cards and the 자동 업데이트 card with the three
/// switches, no checkbox and no ⋯ menu; each item one line — its disclosure, its pill and at most one prominent button (Claude's
/// update, Codex's sign-in, the missing items' install, Tailscale's page) — and none of the Mac's items; each control reaches the
/// host.
#[test]
fn the_components_tab_draws_a_line_per_item_and_its_controls_reach_the_host() {
    common::korean();
    let actions = Arc::new(Recorded::default());
    let surfaces = components_surface(smoke_screen(), updates_model(), actions.clone(), ComponentsView::default());
    let page = pushed(&surfaces, "box");
    let v = &page["view"];
    for aid in [
        "components-header",
        "components-list",
        "components-automatic",
        "cli-auto-update",
        "plugin-auto-update",
        "cli-auto-login",
        "cli-update-progress",
        "cli-update-start",
        "components-recheck",
        "components-group-agents",
        "components-group-tools",
        "components-group-plugins",
        "components-group-automatic",
        "plugin-update-result-claude",
    ] {
        assert!(by_aid(v, aid).is_some(), "{aid}");
    }
    let text = page.to_string();
    assert!(text.contains(&strings::get("settings.components.updateAllButton")), "모두 업데이트");
    assert_eq!(by_aid(v, "cli-update-start").unwrap()["accent"], true, "모두 업데이트 is the header's prominent button");
    assert!(!text.contains("setup-check-") && !text.contains("setup-install"), "no checklist, no bulk install");
    let mut all = vec![];
    nodes(v, &mut all);
    assert!(!text.contains("cli-account-more-") && !all.iter().any(|n| n["t"] == "menu"), "no ⋯ menu");
    assert!(!text.contains("components-panel-"), "every row starts closed");
    assert!(!text.contains("cli-update-result-"), "the results are in the opened rows");
    for provider in ["claude", "codex", "gemini", "pi", "opencode", "hermes"] {
        assert!(by_aid(v, &format!("cli-account-row-{provider}")).is_some(), "{provider}'s row");
    }
    for id in ["git", "node", "uv", "tailscale", "gh", "glab", "ouroboros", "omc"] {
        assert!(by_aid(v, &format!("components-row-{id}")).is_some(), "{id}'s row");
    }
    for id in ["homebrew", "bun", "gstack"] {
        assert!(by_aid(v, &format!("components-row-{id}")).is_none(), "the Mac's {id} is not listed on Windows");
    }
    let toggle = by_aid(v, "components-row-toggle-claude").expect("Claude's disclosure");
    assert_eq!(toggle["expanded"], false, "a disclosure says it is closed");
    assert!(toggle.to_string().contains("▸") && toggle.to_string().contains("2.0.0 (Claude Code)"), "▸, the version: {toggle}");
    // Claude: signed in (who, on the line), out of date: the pill says so and its update (the CLI update) is the line's button.
    assert!(text_of(v, "cli-account-status-claude").contains("me@example.com"));
    assert_eq!(text_of(v, "components-status-claude"), strings::get("settings.components.statusUpdateAvailable"));
    assert_eq!(by_aid(v, "components-update-claude").unwrap()["accent"], true);
    assert!(by_aid(v, "cli-account-change-claude").is_none() && by_aid(v, "cli-account-bedrock-claude").is_none(), "in the panel");
    // Codex: there and signed out: the pill is the account's state, the sign-in the line's button.
    assert_eq!(text_of(v, "cli-account-status-codex"), strings::get("settings.cliAccounts.summarySignedOut"));
    assert_eq!(by_aid(v, "cli-account-login-codex").unwrap()["accent"], true);
    // Antigravity missing: 설치 안 됨 and 설치 instead of the sign-in.
    assert_eq!(text_of(v, "cli-account-status-gemini"), strings::get("settings.cliAccounts.statusNotInstalled"));
    assert!(by_aid(v, "components-install-agy").is_some() && by_aid(v, "cli-account-login-gemini").is_none());
    // A missing tool: its pill and its install; what only a page installs offers its page.
    assert_eq!(text_of(v, "components-status-git"), strings::get("settings.cliAccounts.statusNotInstalled"));
    assert!(by_aid(v, "components-install-git").is_some() && by_aid(v, "components-page-tailscale").is_some());
    // At most one button on a line.
    for provider in ["claude", "codex", "gemini", "pi", "opencode", "hermes"] {
        let buttons = controls_in(v, &format!("cli-account-row-{provider}"));
        assert!(buttons.len() <= 2, "{provider}: the disclosure and at most one button: {buttons:?}");
    }
    let filter = by_aid(v, "components-filter").expect("the filter");
    let problems = filter["options"][1]["label"].as_str().unwrap().to_string();
    assert_eq!(filter["options"][0]["label"], strings::get("settings.components.filterAll"));
    assert!(problems.starts_with("문제 있음 ") && problems != "문제 있음 0", "{problems}");

    for aid in [
        "components-install-git",
        "components-install-agy",
        "components-update-claude",
        "components-page-tailscale",
        "components-recheck",
        "cli-update-start",
        "cli-account-login-codex",
        "components-row-toggle-claude",
    ] {
        press(&surfaces, v, aid);
    }
    surfaces.act("box", filter["options"][1]["id"].as_i64().unwrap(), "toggle", &Value::Null).run(|_| {});
    let said = actions.all();
    assert_eq!(said.len(), 9, "{said:?}");
    assert_eq!(said[..3], ["install git", "install agy", "update"], "an install is its item's; Claude's update is the CLI update");
    assert!(said[3].starts_with("page https://"), "{said:?}");
    assert_eq!(said[4..], ["recheck", "start", "sign in codex", "toggle claude", "problems true"], "새로고침, 모두 업데이트, the sign-in");
}

/// An opened row: the line keeps its pill and gives its button to the raised panel under it, whose lines say what is out of date,
/// the last update's results and what the item is for, over every action of the item, the most needed first (prominent), in one
/// line that wraps; each reaches the host. The results are said under their rows, in the rows' order, once the rows are open.
#[test]
fn an_opened_row_shows_its_lines_and_every_action() {
    common::korean();
    let actions = Arc::new(Recorded::default());
    let surfaces = components_surface(smoke_screen(), updates_model(), actions.clone(), opened(&["claude", "git"]));
    let page = pushed(&surfaces, "box");
    let v = &page["view"];
    assert_eq!(by_aid(v, "components-row-toggle-claude").unwrap()["expanded"], true);
    assert!(by_aid(v, "components-row-toggle-claude").unwrap().to_string().contains("▾"));
    let panel = by_aid(v, "components-panel-claude").expect("Claude's panel").to_string();
    assert!(panel.contains("cli-update-result-claude-updated") && panel.contains("cli-update-result-claude-failed"), "{panel}");
    let actions_line = by_aid(v, "components-actions-claude").unwrap();
    assert_eq!(actions_line["t"], "stack");
    assert_eq!(actions_line["dir"], "h", "the buttons wrap on the page's horizontal stack");
    let mut expected = vec![
        "components-update-claude",
        "cli-account-change-claude",
        "cli-account-logout-claude",
        "cli-account-reset-models-claude",
        "cli-account-bedrock-claude",
    ];
    if mighty_run::os::windows() {
        expected.push("cli-account-reset-bedrock-claude");
    }
    expected.push("cli-account-refresh-claude");
    assert_eq!(controls_in(v, "components-actions-claude"), expected);
    assert_eq!(by_aid(v, "components-update-claude").unwrap()["accent"], true, "the most needed first, prominent");
    assert!(by_aid(v, "cli-account-change-claude").unwrap().get("accent").is_none());
    assert_eq!(controls_in(v, "cli-account-row-claude")[0], "components-row-toggle-claude");
    // Git opened: its install in the panel, not on its line.
    assert_eq!(controls_in(v, "components-actions-git"), ["components-install-git"]);
    for aid in ["cli-account-change-claude", "cli-account-logout-claude", "cli-account-refresh-claude", "components-install-git"] {
        press(&surfaces, v, aid);
    }
    assert_eq!(actions.all(), ["change claude", "logout claude", "refresh claude", "install git"]);

    // Every row open: each result under its CLI's row, in the rows' order; closed, none.
    let all_open = built_components(ComponentsView { all_open: true, ..ComponentsView::default() });
    assert_eq!(mighty_updates::view::shown_statuses(&all_open), ["updated", "failed", "current", "skipped"]);
    assert!(mighty_updates::view::shown_statuses(&built_components(ComponentsView::default())).is_empty());
}

/// 문제 있음: only the rows with a problem (missing, signed out, out of date, failed, needing a check), as many as the filter says;
/// a group without one is left out, and the 자동 업데이트 card stays.
#[test]
fn the_problems_filter_shows_only_the_rows_with_a_problem() {
    common::korean();
    let actions = Arc::new(Recorded::default());
    // Git found: its row has no problem.
    let screen = || {
        let screen = smoke_screen();
        let found = mighty_tools::setup::probe::SetupProbeResult::installed(Some("2.47.1"));
        screen.lock().unwrap().results.insert("git".into(), found);
        screen
    };
    let all = pushed(&components_surface(screen(), updates_model(), actions.clone(), ComponentsView::default()), "box");
    assert_eq!(text_of(&all["view"], "components-status-git"), strings::get("setup.status.installed"));
    let view = ComponentsView { problems_only: true, ..ComponentsView::default() };
    let page = pushed(&components_surface(screen(), updates_model(), actions, view), "box");
    let v = &page["view"];
    let filter = by_aid(v, "components-filter").unwrap();
    assert_eq!(filter["options"][1]["on"], true);
    let count: usize = filter["options"][1]["label"].as_str().unwrap().trim_start_matches("문제 있음 ").parse().unwrap();
    let rows = |v: &Value| {
        let mut all = vec![];
        nodes(v, &mut all);
        all.into_iter()
            .filter_map(|n| n["aid"].as_str().map(str::to_string))
            .filter(|aid| aid.starts_with("components-row-toggle-"))
            .collect::<Vec<_>>()
    };
    let shown = rows(v);
    assert_eq!(shown.len(), count, "{shown:?}");
    assert!(shown.len() < rows(&all["view"]).len(), "a row without a problem is left out");
    for ok in [strings::get("setup.status.installed"), strings::get("settings.components.statusInstalled")] {
        assert!(!page.to_string().contains(&format!("\"text\":\"{ok}\"")), "no {ok} row");
    }
    assert!(shown.contains(&"components-row-toggle-claude".to_string()) && shown.contains(&"components-row-toggle-codex".to_string()));
    assert!(!shown.contains(&"components-row-toggle-git".to_string()), "Git is there");
    assert!(by_aid(v, "components-automatic").is_some() && by_aid(v, "plugin-update-result-claude").is_none());
}

/// While an install runs: its item turns its ring in place of its buttons, every other row's install waits, and a row's install
/// plans that item alone (with the prerequisites it is missing), the screen's own picks left as they are.
#[test]
fn a_rows_install_is_its_items_alone_and_waits_while_one_runs() {
    common::korean();
    let screen = smoke_screen();
    {
        let mut screen = screen.lock().unwrap();
        let picks = screen.selection.clone();
        assert!(screen.may_install("gh") && !screen.may_install("codex"), "a missing item installs, one that is there does not");
        let plan = screen.plan_for(&["gh".to_string()].into());
        assert!(plan.contains(&"gh".to_string()) && !plan.contains(&"agy".to_string()), "{plan:?}");
        let files = mighty_platform::setup::FakeSetupFiles::default();
        let confirmation = screen.may_confirm_item("gh", &files, &Default::default()).expect("gh's confirmation");
        assert!(confirmation.steps.iter().all(|step| plan.contains(&step.item_id)) && confirmation.steps.iter().any(|s| s.item_id == "gh"));
        assert!(screen.may_confirm_item("codex", &files, &Default::default()).is_none(), "nothing to install");
        assert_eq!(screen.selection, picks, "the setup screen's picks are its own");
        let now = std::time::Instant::now();
        screen.run = Some(screen::SetupRun {
            items: vec!["git".into()],
            started: "started".into(),
            done: "done".into(),
            opened: now,
            probed: now,
            finished: false,
            not_started: true,
        });
        assert!(!screen.may_install("gh"), "one install at a time");
    }
    let actions = Arc::new(Recorded::default());
    let surfaces = components_surface(screen, updates_model(), actions, ComponentsView::default());
    let page = pushed(&surfaces, "box");
    let v = &page["view"];
    assert_eq!(text_of(v, "components-status-git"), strings::get("setup.status.installing"));
    assert!(by_aid(v, "components-install-git").is_none(), "the running item has no install");
    let gh = by_aid(v, "components-install-gh").expect("gh's install");
    assert!(gh.to_string().contains("\"disabled\":true"), "another row's install waits: {gh}");
    assert!(by_aid(v, "setup-run-not-started").is_some() && by_aid(v, "setup-run-end").is_some(), "the terminal hint with its 끝내기");
}

fn entry(id: &str, install: ToolkitInstallSpec) -> ToolkitFileEntry {
    ToolkitFileEntry::new(id, &format!("{id} tool"), "package", install)
}

/// The toolkit box: the bundled row with its capsule, a user's tool with its approve and remove, the probe's words, the error line
/// only while there is one, the results row once there are results, and the buttons that wait while an install runs.
#[test]
fn the_toolkit_box_draws_its_rows_error_and_results() {
    common::korean();
    let actions = Arc::new(Recorded::default());
    // A stored entry marked bundled (none ships now, the box still draws one as such).
    let bundled = ToolkitFileEntry {
        source: ToolkitEntrySource::Bundled,
        ..entry("mighty-styles", ToolkitInstallSpec::Plugin { source: "owner/repo".into(), plugin_id: "mighty-styles@market".into() })
    };
    let npm = entry("smoke-npm", ToolkitInstallSpec::Package { manager: "npm".into(), name: "x".into(), executable: None });
    let mut model = ToolkitModel {
        rows: vec![
            ToolkitRow { entry: bundled, approved: false, installed: true },
            ToolkitRow { entry: npm.clone(), approved: false, installed: false },
        ],
        ..ToolkitModel::default()
    };
    let shown = Arc::new(Mutex::new(model.clone()));
    let (state, into) = (shown.clone(), actions.clone());
    let surfaces = Surfaces::new();
    surfaces.open(
        "box",
        Place::Popover,
        Box::new(BoxModel(Box::new(move |ui| view::toolkit_box(ui, &state.lock().unwrap(), into.clone())))),
    );
    let page = pushed(&surfaces, "box");
    let v = &page["view"];
    let rows = v["sections"][0]["node"]["c"].as_array().unwrap().clone();
    assert_eq!(rows.len(), 3, "the hidden error and results rows are dropped: the two tools and the footer");
    assert!(by_aid(v, "toolkit-entry-mighty-styles").unwrap().to_string().contains(&strings::get("settings.toolkit.bundledBadge")));
    assert!(
        by_aid(v, "toolkit-approve-mighty-styles").is_none() && by_aid(v, "toolkit-remove-mighty-styles").is_none(),
        "the bundled tool has no buttons"
    );
    let user = by_aid(v, "toolkit-entry-smoke-npm").unwrap().to_string();
    assert!(
        user.contains(&strings::get("settings.toolkit.needsApproval")) && user.contains(&strings::get("settings.toolkit.statusMissing"))
    );
    for aid in [
        "toolkit-approve-smoke-npm",
        "toolkit-remove-smoke-npm",
        "settings-toolkit-add",
        "settings-toolkit-export",
        "settings-toolkit-import",
        "settings-toolkit-install",
    ] {
        let node = by_aid(v, aid).unwrap_or_else(|| panic!("{aid}"));
        assert!(surfaces.act("box", control_id(&node), "invoke", &Value::Null).run(|_| {}), "{aid}");
    }
    assert_eq!(actions.all(), ["approve smoke-npm", "remove smoke-npm", "add", "export", "import", "toolkit install"]);
    model.error = Some(strings::get("toolkit.error.unreadable"));
    model.running = true;
    model.results = Some(vec![
        ToolkitRunItem { entry_id: "smoke-npm".into(), verdict: Verdict::Installed, steps: vec![] },
        ToolkitRunItem { entry_id: "gone".into(), verdict: Verdict::Failed, steps: vec![] },
    ]);
    *shown.lock().unwrap() = model;
    let page = pushed(&surfaces, "box");
    let v = &page["view"];
    let text = v.to_string();
    assert!(text.contains(&presentation::error_banner(&strings::get("toolkit.error.unreadable"))), "the error line says why");
    let results = by_aid(v, "settings-toolkit-results").expect("the results row").to_string();
    assert!(
        results.contains("smoke-npm tool") && results.contains("gone") && results.contains(&strings::get("settings.toolkit.verdictFailed"))
    );
    let install = by_aid(v, "settings-toolkit-install").unwrap();
    assert!(install["disabled"] == true && install["label"] == strings::get("settings.toolkit.installingButton"), "{install}");
    assert_eq!(by_aid(v, "toolkit-remove-smoke-npm").unwrap()["disabled"], true, "the rows' buttons wait with them");
}

/// D7: the toolkit's confirmation names each tool over every command, a skipped one over its one line.
#[test]
fn the_toolkit_confirmation_names_each_tool_over_its_commands() {
    common::korean();
    let npm = entry("a", ToolkitInstallSpec::Package { manager: "npm".into(), name: "x".into(), executable: None });
    let plan = vec![
        ToolkitPlanItem {
            entry: npm.clone(),
            action: PlanAction::Run,
            commands: vec![vec!["npm".into(), "install".into(), "-g".into(), "x".into()]],
        },
        ToolkitPlanItem { entry: entry("b", npm.install.clone()), action: PlanAction::Skip, commands: vec![] },
    ];
    let surfaces = Surfaces::new();
    surfaces.open(
        "dialog-2",
        Place::Dialog,
        Box::new(DialogSurface::new(view::toolkit_confirmation(presentation::plan_rows(&plan)), |_| {})),
    );
    let page = pushed(&surfaces, "dialog-2");
    let v = &page["view"];
    assert_eq!(v["title"], strings::get("settings.toolkit.confirmTitle"));
    let text = v.to_string();
    assert!(text.contains("npm install -g x") && text.contains("a tool") && text.contains("b tool"));
    assert!(text.contains(&format!("→ {}", strings::get("settings.toolkit.verdictSkipped"))) && text.contains("\"maxH\":300"));
    assert_eq!(v["buttons"][1]["label"], strings::get("settings.toolkit.confirmInstall"));
    assert_eq!(v["buttons"][1]["accent"], false, "no default button");
}
