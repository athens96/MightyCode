//! Settings › 구성 요소 (the owner's layout C: one line per item, opened in place): what was the CLI tab (CLI 계정, CLI 업데이트) and
//! the setup list in one place.
//!
//! - The header line: the filter [전체 | 문제 있음 N] (N: the items missing, signed out, out of date, failed or needing a check), the
//!   refresh (the setup's probe again) and 모두 업데이트 (the manual CLI update), and what the setup says (a refusal, the terminal that
//!   never started) under it.
//! - 에이전트 CLI, 도구 and Claude 플러그인: each group's heading over a card of one-line rows: the disclosure chevron (▸ closed, ▾
//!   open), the item's mark, its name, its version, an agent CLI's account, then at the right a status pill (설치됨/준비됨, 업데이트
//!   있음, 로그인되지 않음, 설치 안 됨, 실패, 확인 필요…) and at most one prominent button, the most needed action (설치, 로그인,
//!   업데이트, or the vendor page of what only a page installs). The line's left part (`components-row-toggle-<id>`) opens the row:
//!   a raised panel under it with the detail lines (what is out of date, the last update's results, the account's detail, why it
//!   needs a check, what the item is for) and every action of the item as buttons that wrap (설치, 업데이트, 계정 변경, 로그아웃, 모델
//!   초기화, Bedrock 설정, Windows' Bedrock 초기화, 대기 취소, 상태 새로고침, 내려받기 페이지 열기), the most needed one first.
//! - 자동 업데이트: a card of one wrapping line, the automatic CLI update's, plugin update's and sign-in's switches (each explained
//!   in its tooltip) and the last run's words at the right.
//!
//! Which rows are open and the filter are the host's ([`ComponentsView`], kept while the app runs); every row starts closed. An
//! install is one item's (plus the prerequisites it is missing) through the setup screen's confirmation and terminal; the update of
//! an agent CLI is the manual CLI update; the sign-in buttons are the accounts'. The first-run setup screen keeps its checklist.

use std::collections::BTreeSet;
use std::sync::Arc;

use mighty_accounts::accounts::{CliAccountStatus, copy as account_copy};
use mighty_accounts::view::{self as accounts_view, AccountActions, AccountRow, AccountsModel};
use mighty_core::design_tokens::{Token, metrics};
use mighty_native::kit::{ControlSize, TextStyle};
use mighty_native::tokens::Brush;
use mighty_native::ui::{Content, El, Grid, HAlign, Kind, Thickness, Track, Ui, VAlign, Wrapping};
use mighty_run::{providers, strings};
use mighty_updates::view::{self as updates, CliUpdateActions, CliUpdateBox};

use super::{glyph, notices, ring, text};
use crate::setup::catalog::{SetupCatalog, SetupGroup, SetupItem, SetupPlatform, SetupProbe, SetupRecipe};
use crate::setup::probe::{SetupProbeState, SetupRowState, SetupRows};
use crate::setup::screen::SetupScreen;

/// The automation id of the tab's rows (`components-list`), the prefix of a tool's or plugin's row (`components-row-<id>`), of a
/// row's disclosure (`components-row-toggle-<id>`) and the filter's (`components-filter`).
pub const LIST_ID: &str = "components-list";
pub const ROW_ID_PREFIX: &str = "components-row-";
pub const TOGGLE_ID_PREFIX: &str = "components-row-toggle-";
pub const FILTER_ID: &str = "components-filter";
/// The refresh mark (Segoe Fluent `Refresh`).
const REFRESH_GLYPH: &str = "\u{E72C}";

/// What the tab's own controls do (the accounts' and the updates' buttons do what their actions say).
pub trait ComponentActions: Send + Sync {
    /// 설치 or 업데이트 of one item: that item alone (with the prerequisites it is missing) through the setup's confirmation.
    fn install(&self, id: &str);
    /// 업데이트 of an out-of-date agent CLI: the manual CLI update (as the setup screen's).
    fn update(&self);
    /// What only a page installs: its page.
    fn open_page(&self, url: &str);
    /// 새로고침: PATH read again, every item probed.
    fn recheck(&self);
    /// The terminal hint's 끝내기: the run that never started closed by hand.
    fn abandon(&self);
    /// A row's line pressed: that row opened, or closed when it was open.
    fn toggle(&self, id: &str);
    /// The filter: only the rows with a problem (true) or every row.
    fn show_problems(&self, only: bool);
}

/// What the tab shows of itself: the rows opened and the filter, the host's while the app runs.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ComponentsView {
    /// The opened rows, by item id (an agent CLI's by its provider before the catalog is read).
    pub open: BTreeSet<String>,
    /// 문제 있음: only the rows with a problem.
    pub problems_only: bool,
    /// Every row open (a check that reads every row's lines).
    pub all_open: bool,
}

impl ComponentsView {
    pub fn is_open(&self, id: &str) -> bool {
        self.all_open || self.open.contains(id)
    }

    /// The row `id` opened, or closed when it was open.
    pub fn toggle(&mut self, id: &str) {
        if !self.open.remove(id) {
            self.open.insert(id.to_string());
        }
    }
}

/// What the tab draws from: the setup screen's state, the accounts' rows (each provider's status, wait, message and work, the
/// automatic sign-in switch), the CLI updates' (the switches, what runs, the results) and its own ([`ComponentsView`]).
pub struct ComponentsModel<'a> {
    pub screen: &'a SetupScreen,
    pub accounts: &'a AccountsModel,
    pub updates: &'a CliUpdateBox,
    pub view: &'a ComponentsView,
}

/// The actions of every control of the tab.
#[derive(Clone)]
pub struct ComponentsActions {
    pub components: Arc<dyn ComponentActions>,
    pub accounts: Arc<dyn AccountActions>,
    pub updates: CliUpdateActions,
}

/// The tab's three groups, in their order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Group {
    Agents,
    Tools,
    Plugins,
}

impl Group {
    pub const ALL: [Group; 3] = [Group::Agents, Group::Tools, Group::Plugins];

    /// An item's group: an agent CLI's, a Claude plugin's or skill's, else a tool's (Git, Homebrew, Node, Bun, uv, Tailscale, gh,
    /// glab).
    pub fn of(screen: &SetupScreen, item: &SetupItem) -> Group {
        if item.group == SetupGroup::Agent || item.provider().is_some() {
            return Group::Agents;
        }
        match item.entry(screen.platform()).map(|entry| &entry.probe) {
            Some(SetupProbe::ClaudePlugin { .. } | SetupProbe::Skill { .. }) => Group::Plugins,
            _ => Group::Tools,
        }
    }

    /// The group's heading's automation id: `components-group-<id>`.
    pub fn id(self) -> &'static str {
        match self {
            Group::Agents => "agents",
            Group::Tools => "tools",
            Group::Plugins => "plugins",
        }
    }

    pub fn title_key(self) -> &'static str {
        match self {
            Group::Agents => "setup.group.agent",
            Group::Tools => "settings.components.groupTools",
            Group::Plugins => "settings.components.groupPlugins",
        }
    }
}

/// A status pill's colours.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tone {
    /// 설치됨, 준비됨: `done-text` on `done-soft`.
    Ok,
    /// 업데이트 있음, 확인 필요: `wait-text` on `wait-soft`.
    Warn,
    /// 실패: `err-text` on `err-soft`.
    Err,
    /// 설치 안 됨, 로그인되지 않음, 확인 중: `ink2` on `track`.
    Neutral,
    /// 설치 중, 로그인 대기 중: `accent` on `accent-soft`.
    Busy,
}

fn pill(ui: &Ui, words: String, tone: Tone) -> El {
    let (ink, fill) = match tone {
        Tone::Ok => (Token::DoneText, Token::DoneSoft),
        Tone::Warn => (Token::WaitText, Token::WaitSoft),
        Tone::Err => (Token::ErrText, Token::ErrSoft),
        Tone::Neutral => (Token::Ink2, Token::Track),
        Tone::Busy => (Token::Accent, Token::AccentSoft),
    };
    ui.settings_pill(words, ink, fill)
}

/// The most needed action of a row, the one its line offers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Need {
    Install,
    SignIn,
    Update,
    Page,
}

/// What a row says on its line: its pill, whether it is a problem (the filter's), whether its ring turns, its most needed action.
struct Facts {
    words: String,
    tone: Tone,
    problem: bool,
    turning: bool,
    need: Option<Need>,
}

/// The vendor page of what only a page installs here.
fn vendor_page(item: &SetupItem, platform: SetupPlatform) -> Option<String> {
    match item.entry(platform).map(|entry| &entry.install) {
        Some(SetupRecipe::VendorPage { url }) => Some(url.to_string()),
        _ => None,
    }
}

/// The setup's actions an item's state offers, in order: 설치 while missing (or failed) and installable here, 업데이트 while out
/// of date (an agent CLI's the CLI update unless its install updates it), the vendor page of what only a page installs while it is
/// not installed; none while it installs.
fn setup_needs(screen: &SetupScreen, item: &SetupItem, state: SetupRowState) -> Vec<Need> {
    let platform = screen.platform();
    let installs = SetupRows::is_selectable_on(state, item, platform);
    let mut needs = vec![];
    match state {
        SetupRowState::Installing | SetupRowState::Checking => return needs,
        SetupRowState::Missing | SetupRowState::Failed if installs => needs.push(Need::Install),
        SetupRowState::Outdated if installs || item.provider().is_some() => needs.push(Need::Update),
        _ => {}
    }
    if state != SetupRowState::Installed && vendor_page(item, platform).is_some() {
        needs.push(Need::Page);
    }
    needs
}

/// The line of a setup state: its words, tone and whether it is a problem.
fn setup_words(state: SetupRowState) -> (String, Tone, bool) {
    match state {
        SetupRowState::Installed => (strings::get("setup.status.installed"), Tone::Ok, false),
        SetupRowState::Missing => (account_copy::status_not_installed(), Tone::Neutral, true),
        SetupRowState::Outdated => (strings::get("settings.components.statusUpdateAvailable"), Tone::Warn, true),
        SetupRowState::Attention => (strings::get("setup.status.attention"), Tone::Warn, true),
        SetupRowState::Installing => (strings::get("setup.status.installing"), Tone::Busy, false),
        SetupRowState::Failed => (strings::get("setup.status.failed"), Tone::Err, true),
        SetupRowState::Checking => (strings::get("setup.status.checking"), Tone::Neutral, false),
    }
}

/// A tool's or plugin's line: its setup state, its first setup action (the vendor page only while it is missing).
fn item_facts(screen: &SetupScreen, item: &SetupItem) -> Facts {
    let state = screen.state_of(item);
    let (words, tone, problem) = setup_words(state);
    let need = setup_needs(screen, item, state)
        .into_iter()
        .find(|need| *need != Need::Page || matches!(state, SetupRowState::Missing | SetupRowState::Failed));
    Facts { words, tone, problem, turning: state == SetupRowState::Installing, need }
}

/// An agent CLI's account row as 구성 요소 reads it with the setup's state: a CLI the probe finds missing has no account (its 설치
/// stands for the sign-in), one it finds while the accounts have not seen it yet reads as being checked.
fn agent_account(model: &ComponentsModel<'_>, provider: &str, state: Option<SetupRowState>) -> AccountRow {
    let checking = || CliAccountStatus { detail: account_copy::status_checking(), ..CliAccountStatus::new(provider) };
    let mut row = model.accounts.rows.iter().find(|row| row.status.provider == provider).cloned().unwrap_or_else(|| AccountRow {
        status: checking(),
        pending: false,
        message: None,
        working: false,
    });
    match state {
        Some(SetupRowState::Missing | SetupRowState::Failed | SetupRowState::Installing) => row.status.installed = false,
        Some(SetupRowState::Installed | SetupRowState::Outdated | SetupRowState::Attention) if !row.status.installed => {
            row.status = checking()
        }
        _ => {}
    }
    row
}

/// An agent CLI's line: the setup's state while it is not usable (missing, failed, installing, being probed, needing a check), else
/// its account's (로그인되지 않음, 로그인 대기 중, 확인 중, 확인 필요), else 업데이트 있음 or 준비됨. Its most needed action: 설치
/// (or the page), else 로그인 while signed out, else 업데이트.
fn agent_facts(model: &ComponentsModel<'_>, item: Option<&SetupItem>, row: &AccountRow) -> Facts {
    let state = item.map(|item| model.screen.state_of(item));
    let needs = item.zip(state).map(|(item, state)| setup_needs(model.screen, item, state)).unwrap_or_default();
    let status = &row.status;
    let checking = status.logged_in.is_none() && status.detail == account_copy::status_checking();
    let turning = model.accounts.reading || row.working || checking || state == Some(SetupRowState::Installing);
    let setup = |state: SetupRowState| {
        let (words, tone, problem) = setup_words(state);
        Facts { words, tone, problem, turning, need: needs.first().copied() }
    };
    match state {
        // While the probe is still looking, the account already known says more than "checking"; it says checking itself when it
        // knows nothing either.
        Some(state @ (SetupRowState::Missing | SetupRowState::Failed | SetupRowState::Installing)) => {
            return setup(state);
        }
        Some(SetupRowState::Checking) if checking => return setup(SetupRowState::Checking),
        Some(SetupRowState::Attention) => return setup(SetupRowState::Attention),
        _ => {}
    }
    if !status.installed {
        // No catalog: the accounts alone say it is missing.
        return Facts { words: account_copy::status_not_installed(), tone: Tone::Neutral, problem: true, turning, need: None };
    }
    let update = needs.contains(&Need::Update).then_some(Need::Update);
    if row.pending {
        return Facts {
            words: strings::get("settings.components.statusSigningIn"),
            tone: Tone::Busy,
            problem: false,
            turning,
            need: update,
        };
    }
    match status.logged_in {
        Some(false) => Facts { words: status.summary(), tone: Tone::Neutral, problem: true, turning, need: Some(Need::SignIn) },
        None if checking => Facts { words: status.summary(), tone: Tone::Neutral, problem: false, turning, need: update },
        None => Facts { words: strings::get("setup.status.attention"), tone: Tone::Warn, problem: true, turning, need: update },
        Some(true) if state == Some(SetupRowState::Outdated) => Facts {
            words: strings::get("settings.components.statusUpdateAvailable"),
            tone: Tone::Warn,
            problem: true,
            turning,
            need: update,
        },
        Some(true) => {
            Facts { words: strings::get("settings.components.statusInstalled"), tone: Tone::Ok, problem: false, turning, need: None }
        }
    }
}

/// A row's push button: Regular, prominent when it is the row's most needed action.
fn push(ui: &Ui, key: &str, aid: String, prominent: bool, enabled: bool, action: impl Fn() + Send + Sync + 'static) -> El {
    let button = ui.button(strings::get(key), action).enabled(enabled);
    ui.settings_push(button, ControlSize::Regular, prominent, false).aid(aid)
}

/// The button of one of the setup's actions of `item` (`components-install-<id>`, `components-update-<id>`, `components-page-<id>`).
/// An install waits while another runs (one at a time); an agent CLI's update is the CLI update, which waits while one or a plugin
/// operation runs.
fn setup_button(ui: &Ui, model: &ComponentsModel<'_>, item: &SetupItem, need: Need, prominent: bool, actions: &ComponentsActions) -> El {
    let screen = model.screen;
    let running = screen.running();
    let installs = SetupRows::is_selectable_on(screen.state_of(item), item, screen.platform());
    let (act, id) = (actions.components.clone(), item.id.clone());
    match need {
        Need::Install => {
            push(ui, "settings.components.installButton", format!("components-install-{id}"), prominent, !running, move || act.install(&id))
        }
        Need::Update if installs => {
            push(ui, "setup.button.update", format!("components-update-{id}"), prominent, !running, move || act.install(&id))
        }
        Need::Update => {
            let busy = model.updates.any_running;
            let key = if busy { "settings.components.inProgressLabel" } else { "setup.button.update" };
            let enabled = !busy && !model.updates.plugin_operation_running;
            push(ui, key, format!("components-update-{id}"), prominent, enabled, move || act.update())
        }
        Need::Page | Need::SignIn => {
            let url = vendor_page(item, screen.platform()).unwrap_or_default();
            push(ui, "setup.button.openPage", format!("components-page-{id}"), prominent, true, move || act.open_page(&url))
        }
    }
}

/// The lines of an item's opened row from the setup: what is out of date or unread, why it needs attention.
fn setup_lines(ui: &Ui, screen: &SetupScreen, item: &SetupItem) -> Vec<El> {
    let mut lines = vec![];
    let Some(result) = screen.results.get(&item.id) else { return lines };
    let wait = |words: String| text(ui, words, TextStyle::size(11.0).ink(Token::WaitText));
    match (result.state, result.minimum.as_deref()) {
        (SetupProbeState::Outdated, Some(minimum)) => lines.push(wait(strings::get_with(
            "setup.outdatedDetail",
            &[("version", result.version.as_deref().unwrap_or("?")), ("minimum", minimum)],
        ))),
        (SetupProbeState::Unverified, Some(minimum)) => {
            lines.push(wait(strings::get_with("setup.unverifiedDetail", &[("minimum", minimum)])))
        }
        _ => {}
    }
    if let Some(reason) = result.attention.filter(|_| result.state == SetupProbeState::NeedsAttention) {
        lines.push(wait(strings::get(reason)));
    }
    lines
}

/// What an item is for, under its opened row's other lines.
fn description(ui: &Ui, screen: &SetupScreen, item: &SetupItem) -> El {
    text(ui, SetupCatalog::detail_on(item, screen.platform()), TextStyle::size(11.0).ink(Token::Ink2))
}

/// The lines of an agent CLI's account in its opened row (who is signed in is on its line): the account's detail (when its line does
/// not say it), the pending sign-in, what a button of the row said.
fn account_lines(ui: &Ui, row: &AccountRow, said: &str) -> Vec<El> {
    let status = &row.status;
    let provider = &status.provider;
    let mut lines = vec![];
    if status.installed && !status.detail.is_empty() && status.detail != status.summary() && status.detail != said {
        lines.push(text(ui, status.detail.clone(), TextStyle::size(11.0).ink(Token::Ink2).selectable()));
    }
    if row.pending {
        lines.push(text(ui, account_copy::status_pending(), TextStyle::size(11.0).ink(Token::Ink2)));
    }
    if let Some(message) = &row.message {
        let warning = ui.settings_symbol(glyph::WARNING, 11.0, Token::WaitText).va(VAlign::Top).margin(Thickness::new(0.0, 2.0, 0.0, 0.0));
        let words = text(ui, message.clone(), TextStyle::size(11.0).ink(Token::WaitText)).cell(0, 1);
        lines.push(
            El::grid(Grid {
                children: vec![warning, words],
                cols: vec![Track::Auto, Track::Star(1.0)],
                col_spacing: metrics::SPACING_XS,
                ..Grid::default()
            })
            .aid(format!("cli-account-message-{provider}")),
        );
    }
    lines
}

/// The parts of one row: its key (the toggle's and the view's), its mark, its name's parts, the words after the name (an agent's
/// account), the pill (and the pill's automation id), its line's button, the opened panel's lines and buttons.
struct RowParts {
    id: String,
    title: String,
    aid: String,
    mark: El,
    name: Vec<El>,
    account: Option<El>,
    facts: Facts,
    pill_aid: String,
    primary: Option<El>,
    lines: Vec<El>,
    buttons: Vec<El>,
}

/// One row: its line — the disclosure (chevron, mark, name, version, account: a plain button, `components-row-toggle-<id>`), the
/// ring while something runs, the pill and the line's button — and, opened, the raised panel of its lines over its buttons, which
/// wrap.
fn row(ui: &Ui, model: &ComponentsModel<'_>, parts: RowParts, actions: &ComponentsActions) -> El {
    let RowParts { id, title, aid, mark, name, account, facts, pill_aid, primary, lines, buttons } = parts;
    let open = model.view.is_open(&id);
    let mut head = vec![text(ui, if open { "▾" } else { "▸" }, TextStyle::size(11.0).ink(Token::Ink2)).width(10.0), mark];
    head.extend(name);
    if let Some(account) = account {
        head.push(account);
    }
    let toggle = actions.components.clone();
    let key = id.clone();
    let mut disclosure = El::new(Kind::Button(mighty_native::ui::Button {
        content: Content::El(Box::new(El::stack(true, metrics::SPACING_SM, head).va(VAlign::Center))),
        expanded: Some(open),
        ..Default::default()
    }))
    .named(title)
    .ha(HAlign::Stretch)
    .on_click(move || toggle.toggle(&key));
    ui.paint_plain_button(&mut disclosure, Brush::TRANSPARENT, Some(ui.brush(Token::Ink)), Some(ui.brush(Token::Ink3)));
    let mut line = vec![disclosure.aid(format!("{TOGGLE_ID_PREFIX}{id}"))];
    if facts.turning {
        line.push(ring(14.0));
    }
    line.push(pill(ui, facts.words, facts.tone).aid(pill_aid));
    if !open && let Some(primary) = primary {
        line.push(primary);
    }
    // The disclosure takes what is left; the ring, the pill and the button their own width.
    let mut cols = vec![Track::Star(1.0)];
    cols.resize(line.len(), Track::Auto);
    let line: Vec<El> = line.into_iter().enumerate().map(|(col, el)| el.cell(0, col as i64)).collect();
    let line = El::grid(Grid { children: line, cols, col_spacing: metrics::SPACING_SM, ..Grid::default() });
    let mut stack = vec![line];
    if open {
        let mut panel = lines;
        if !buttons.is_empty() {
            panel.push(El::stack(true, metrics::SPACING_SM, buttons).aid(format!("components-actions-{id}")));
        }
        let panel = El::border(mighty_native::ui::Border {
            child: Some(Box::new(El::stack(false, metrics::SPACING_SM, panel))),
            background: Some(ui.brush(Token::CardRaised)),
            padding: Thickness::new(metrics::SPACING_MD, 10.0, metrics::SPACING_MD, 10.0),
            corner_radius: 8.0,
            ..Default::default()
        })
        .margin(Thickness::new(18.0, metrics::SPACING_SM, 0.0, 0.0))
        .aid(format!("components-panel-{id}"));
        stack.push(panel);
    }
    El::stack(false, 0.0, stack).aid(aid).keyed(id)
}

/// An agent CLI's mark: the agent symbol.
fn agent_mark(ui: &Ui) -> El {
    ui.settings_symbol(glyph::AGENT, 13.0, Token::AgentText)
}

/// An agent CLI's row (`cli-account-row-<provider>`): its account's line beside the setup's state; opened, the setup's and the
/// account's lines and buttons. Before the catalog is read (`item` None) it is the account alone.
fn agent_row(ui: &Ui, model: &ComponentsModel<'_>, item: Option<&SetupItem>, provider: &str, actions: &ComponentsActions) -> (El, bool) {
    let screen = model.screen;
    let state = item.map(|item| screen.state_of(item));
    let row_state = agent_account(model, provider, state);
    let facts = agent_facts(model, item, &row_state);
    let status = &row_state.status;
    let mut name = vec![text(ui, strings::provider_label(provider), TextStyle::size(13.0).medium())];
    if providers::is_beta(provider) {
        name.push(ui.beta_badge());
    }
    if let Some(version) = item.and_then(|item| screen.results.get(&item.id)).and_then(|result| result.shown_version()) {
        name.push(text(ui, version, TextStyle::size(11.0).ink(Token::Ink2).mono()));
    }
    // Signed in: who, on the line (the accounts' status id); otherwise the pill says the account's state.
    let signed_in = status.installed && status.logged_in == Some(true) && !row_state.pending;
    let account = signed_in.then(|| {
        let summary = status.summary();
        let mut words = text(ui, summary.clone(), TextStyle::size(11.0).ink(Token::Ink2).selectable());
        if let Some(block) = words.text_mut() {
            block.wrapping = Wrapping::NoWrap;
        }
        words.tooltip(summary).aid(format!("cli-account-status-{provider}"))
    });
    let pill_aid = match (signed_in, item) {
        (true, Some(item)) => format!("components-status-{}", item.id),
        (true, None) => format!("components-status-{provider}"),
        (false, _) => format!("cli-account-status-{provider}"),
    };
    let setup = item.zip(state).map(|(item, state)| setup_needs(screen, item, state)).unwrap_or_default();
    let primary_button = |prominent: bool| -> Option<El> {
        match facts.need? {
            Need::SignIn => Some(accounts_view::login_button(ui, &row_state, &actions.accounts, prominent)),
            need => item.map(|item| setup_button(ui, model, item, need, prominent, actions)),
        }
    };
    let primary = primary_button(true);
    // Opened: the most needed action first (prominent), the setup's other actions, then the account's (its sign-in unless it led).
    let mut buttons: Vec<El> = primary_button(true).into_iter().collect();
    if let Some(item) = item {
        for need in setup.iter().copied().filter(|need| Some(*need) != facts.need) {
            buttons.push(setup_button(ui, model, item, need, false, actions));
        }
    }
    let with_login = (facts.need != Some(Need::SignIn)).then_some(false);
    buttons.extend(accounts_view::account_buttons(ui, &row_state, &actions.accounts, with_login));
    let mut lines = account_lines(ui, &row_state, &facts.words);
    if let Some(item) = item {
        lines.extend(setup_lines(ui, screen, item));
    }
    lines.extend(model.updates.results.iter().filter(|result| result.provider == provider).map(|result| updates::result_line(ui, result)));
    if let Some(item) = item {
        lines.push(description(ui, screen, item));
    }
    let problem = facts.problem;
    let id = item.map_or_else(|| provider.to_string(), |item| item.id.clone());
    let parts = RowParts {
        id,
        title: strings::provider_label(provider).to_string(),
        aid: format!("{}{provider}", accounts_view::ACCOUNT_ROW_ID_PREFIX),
        mark: agent_mark(ui),
        name,
        account,
        facts,
        pill_aid,
        primary,
        lines,
        buttons,
    };
    (row(ui, model, parts, actions), problem)
}

/// A tool's or plugin's row (`components-row-<id>`): its mark, its name (with 필수 when it is required) and version, its pill
/// (`components-status-<id>`); opened, what is out of date or needs a check, what it is for, and its setup actions.
fn item_row(ui: &Ui, model: &ComponentsModel<'_>, item: &SetupItem, actions: &ComponentsActions) -> (El, bool) {
    let screen = model.screen;
    let platform = screen.platform();
    let facts = item_facts(screen, item);
    let mark = match item.group {
        SetupGroup::Style => glyph::STYLE,
        SetupGroup::Network => glyph::NETWORK,
        SetupGroup::Dev => glyph::DEV,
        SetupGroup::Agent => glyph::AGENT,
        SetupGroup::Prerequisite => glyph::TOOL,
    };
    let mut name = vec![text(ui, SetupCatalog::title_on(item, platform), TextStyle::size(13.0).medium())];
    if item.required {
        name.push(ui.settings_capsule(
            strings::get("setup.badge.required"),
            ui.brush(Token::Accent),
            ui.brush_at(Token::Accent, 0.15),
            9.0,
            3.0,
        ));
    }
    if let Some(version) = screen.results.get(&item.id).and_then(|result| result.shown_version()) {
        name.push(text(ui, version, TextStyle::size(11.0).ink(Token::Ink2).mono()));
    }
    let state = screen.state_of(item);
    let needs = setup_needs(screen, item, state);
    let primary = facts.need.map(|need| setup_button(ui, model, item, need, true, actions));
    let mut buttons: Vec<El> = facts.need.map(|need| setup_button(ui, model, item, need, true, actions)).into_iter().collect();
    for need in needs.into_iter().filter(|need| Some(*need) != facts.need) {
        buttons.push(setup_button(ui, model, item, need, false, actions));
    }
    let mut lines = setup_lines(ui, screen, item);
    lines.push(description(ui, screen, item));
    let problem = facts.problem;
    let parts = RowParts {
        id: item.id.clone(),
        title: SetupCatalog::title_on(item, platform),
        aid: format!("{ROW_ID_PREFIX}{}", item.id),
        mark: ui.settings_symbol(mark, 13.0, Token::Ink2),
        name,
        account: None,
        facts,
        pill_aid: format!("components-status-{}", item.id),
        primary,
        lines,
        buttons,
    };
    (row(ui, model, parts, actions), problem)
}

/// The header line: the filter at the left, the refresh (a ring beside it while a probe runs) and 모두 업데이트 at the right.
fn header(ui: &Ui, model: &ComponentsModel<'_>, problems: usize, actions: &ComponentsActions) -> El {
    let probing = model.screen.probing;
    let options = [
        ("all".to_string(), strings::get("settings.components.filterAll")),
        ("problems".to_string(), strings::get_with("settings.components.filterProblems", &[("count", &problems.to_string())])),
    ];
    let shown = if model.view.problems_only { "problems" } else { "all" };
    let pick = actions.components.clone();
    let filter = ui.settings_segmented(
        strings::get("settings.components.filterName"),
        FILTER_ID,
        &options,
        shown,
        move |value| pick.show_problems(value == "problems"),
        false,
        true,
    );
    let recheck = actions.components.clone();
    let label = strings::get(if probing { "setup.button.checking" } else { "setup.button.recheck" });
    let check = ui.settings_icon_button(REFRESH_GLYPH, label, move || recheck.recheck(), 14.0).enabled(!probing).aid("components-recheck");
    let all = updates::start_button(ui, model.updates, &actions.updates, strings::get("settings.components.updateAllButton"));
    let all = ui.settings_push(all, ControlSize::Regular, true, false);
    let mut children = vec![filter.ha(HAlign::Left).va(VAlign::Center).cell(0, 0)];
    let mut cols = vec![Track::Auto, Track::Star(1.0)];
    if probing {
        children.push(ring(14.0).cell(0, cols.len() as i64));
        cols.push(Track::Auto);
    }
    children.push(check.cell(0, cols.len() as i64));
    cols.push(Track::Auto);
    children.push(all.cell(0, cols.len() as i64));
    cols.push(Track::Auto);
    El::grid(Grid { children, cols, col_spacing: metrics::SPACING_SM, ..Grid::default() }).aid("components-header")
}

/// The 자동 업데이트 card: the three switches on one wrapping line, each beside its short label with its explanation as the pair's
/// tooltip, and the progress or last-run words at the right.
fn automatic(ui: &Ui, model: &ComponentsModel<'_>, actions: &ComponentsActions) -> El {
    let [(clis, _, clis_help), (plugins, _, plugins_help)] = updates::switches(ui, model.updates, &actions.updates);
    let (login, _, login_help) = accounts_view::auto_login_switch(ui, model.accounts.auto_login, &actions.accounts);
    let pair = |switch: El, key: &str, help: String| {
        El::stack(true, metrics::SPACING_SM, vec![switch, text(ui, strings::get(key), TextStyle::size(13.0))]).tooltip(help)
    };
    let switches = El::stack(
        true,
        metrics::SPACING_LG,
        vec![
            pair(clis, "settings.components.autoUpdateClis", clis_help),
            pair(plugins, "settings.components.autoUpdatePlugins", plugins_help),
            pair(login, "settings.components.autoLogin", login_help),
        ],
    );
    let words = updates::progress_words(ui, model.updates).ha(HAlign::Right).cell(0, 1);
    let line = El::grid(Grid {
        children: vec![switches, words],
        cols: vec![Track::Star(1.0), Track::Auto],
        col_spacing: metrics::SPACING_MD,
        ..Grid::default()
    });
    let mut rows = ui.settings_rows();
    rows.add(line);
    ui.settings_card(rows.finish()).aid("components-automatic")
}

/// A group's heading over its card.
fn heading(ui: &Ui, words: String, aid: String) -> El {
    text(ui, words, TextStyle::size(13.0).medium().ink(Token::Ink2))
        .margin(Thickness::new(metrics::SPACING_LG, metrics::SPACING_MD, 0.0, 0.0))
        .aid(aid)
}

/// The tab's description: where installs and sign-ins run on this platform.
fn description_key(screen: &SetupScreen) -> &'static str {
    match screen.platform() {
        SetupPlatform::MacOS => "settings.components.descriptionMac",
        SetupPlatform::Windows => "settings.components.descriptionWindows",
    }
}

/// Settings › 구성 요소 (marked `cards`: the page draws its box without its own card): the header line and the setup's notices,
/// each group's heading over its card of rows (only this platform's items; a group with none, or with no problem under 문제 있음, is
/// left out), the 자동 업데이트 card and the tab's description. Before the catalog is read the agent CLIs are the accounts' rows.
pub fn components_tab(ui: &mut Ui, model: &ComponentsModel<'_>, actions: &ComponentsActions) -> El {
    let items = model.screen.items();
    let only = model.view.problems_only;
    let mut groups = vec![];
    let mut problems = 0;
    for group in Group::ALL {
        let members: Vec<&SetupItem> = items.iter().copied().filter(|item| Group::of(model.screen, item) == group).collect();
        let mut built = vec![];
        for item in &members {
            built.push(match item.provider() {
                Some(provider) => agent_row(ui, model, Some(item), provider, actions),
                None => item_row(ui, model, item, actions),
            });
        }
        if group == Group::Agents && members.is_empty() {
            // No catalog (yet): the accounts' rows as they are.
            for account in &model.accounts.rows {
                built.push(agent_row(ui, model, None, &account.status.provider, actions));
            }
        }
        problems += built.iter().filter(|(_, problem)| *problem).count();
        let mut rows: Vec<El> = built.into_iter().filter(|(_, problem)| !only || *problem).map(|(row, _)| row).collect();
        if !only {
            if group == Group::Agents {
                // A result of a CLI without a row here (another platform's) is still said.
                let listed: Vec<&str> = members.iter().filter_map(|item| item.provider()).collect();
                rows.extend(
                    model
                        .updates
                        .results
                        .iter()
                        .filter(|result| !listed.contains(&result.provider.as_str()))
                        .map(|result| updates::result_line(ui, result)),
                );
            }
            if group == Group::Plugins {
                rows.extend(
                    model.updates.plugin_results.iter().map(|(provider, result)| updates::plugin_result_line(ui, provider, result)),
                );
            }
        }
        if !rows.is_empty() {
            groups.push((group, rows));
        }
    }

    let mut parts = vec![header(ui, model, problems, actions)];
    let abandon = actions.components.clone();
    parts.extend(notices(ui, model.screen, move || abandon.abandon()));
    if only && groups.is_empty() {
        parts.push(
            text(ui, strings::get("settings.components.noProblems"), TextStyle::size(12.0).ink(Token::Ink2)).aid("components-no-problems"),
        );
    }
    let mut list = vec![];
    for (group, rows) in groups {
        list.push(heading(ui, strings::get(group.title_key()), format!("components-group-{}", group.id())));
        let mut card = ui.settings_rows();
        for row in rows {
            card.add(row);
        }
        list.push(ui.settings_card(card.finish()));
    }
    parts.push(El::stack(false, metrics::SPACING_SM, list).aid(LIST_ID));
    parts.push(heading(ui, strings::get("settings.components.groupAutoUpdate"), "components-group-automatic".into()));
    parts.push(automatic(ui, model, actions));
    parts.push(text(ui, strings::get(description_key(model.screen)), TextStyle::size(11.0).ink(Token::Ink2)).margin(Thickness::new(
        metrics::SPACING_LG,
        metrics::SPACING_XS,
        metrics::SPACING_LG,
        0.0,
    )));
    El::stack(false, metrics::SPACING_SM, parts).mark("cards")
}
