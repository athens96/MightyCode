//! The CLI accounts' rows and dialogs D8 and D9 (C# MainWindow.Settings.cs:628-810 `BuildCliAccountsSection`, :905-932
//! `ConfirmCliSignOut`, :1015-1026 `ResetClaudeBedrock`), built with mighty-native's settings kit from models the host keeps: no I/O
//! here, they run on every read of the settings surface. What a button does is the host's ([`AccountActions`]).
//!
//! Settings has no CLI tab any more: 구성 요소 draws each agent CLI as one line that opens in place (mighty-tools' components view),
//! with this crate's sign-in ([`login_button`]), account buttons ([`account_buttons`]) and automatic sign-in switch
//! ([`auto_login_switch`]). [`accounts`], the whole CLI 계정 box as C# draws it, is kept for its parity golden (tests/golden.rs).

use std::sync::Arc;

use mighty_core::design_tokens::{Token, metrics};
use mighty_native::kit::{ControlSize, TextStyle, provider_mark};
use mighty_native::surface::{DialogButton, DialogSpec};
use mighty_native::ui::{Content, El, Grid, HAlign, Kind, MenuItem, Thickness, Track, Ui, VAlign, Wrapping};
use mighty_run::{providers, strings};

use crate::accounts::{CliAccountStatus, CliLoginOption, SECTION_PROVIDERS, copy};

/// `AccountGap`: the space between the parts of an account's line.
pub const ACCOUNT_GAP: f64 = 8.0;
/// `AccountGive`: the words' and the model reset's weight against the spacer's 1.
pub const ACCOUNT_GIVE: f64 = 1000.0;
/// `AccountRowIdPrefix`.
pub const ACCOUNT_ROW_ID_PREFIX: &str = "cli-account-row-";
/// The warning mark of a row's message (Segoe Fluent `Warning`).
pub const WARNING_GLYPH: &str = "\u{E7BA}";
/// The refresh mark (Segoe Fluent `Refresh`).
pub const REFRESH_GLYPH: &str = "\u{E72C}";

/// One account's row as the section shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountRow {
    pub status: CliAccountStatus,
    /// A terminal sign-in is waited on (`accountLoginWaits`).
    pub pending: bool,
    /// What a button of the row said (`accountMessages`).
    pub message: Option<String>,
    /// A button of the row is at work (`accountRowsWorking`): its ring turns, its buttons wait.
    pub working: bool,
}

/// The CLI accounts box's model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountsModel {
    pub rows: Vec<AccountRow>,
    /// The tab's own reading of the statuses is under way (`settingsAccountRefreshes > 0`).
    pub reading: bool,
    /// The automatic sign-in switch (`AutoLoginCLIs != false`).
    pub auto_login: bool,
}

impl AccountsModel {
    /// `BuildCliAccountsSectionFromState`: each listed provider's last status, else "checking".
    pub fn from_statuses(statuses: &std::collections::HashMap<String, CliAccountStatus>) -> Vec<CliAccountStatus> {
        SECTION_PROVIDERS
            .iter()
            .map(|provider| {
                statuses
                    .get(*provider)
                    .cloned()
                    .unwrap_or_else(|| CliAccountStatus { detail: copy::status_checking(), ..CliAccountStatus::new(provider) })
            })
            .collect()
    }
}

/// What the CLI accounts box's buttons do (each the C# button's action: `ConfirmCliSignOut`, `StartCliSignIn`, `ResetClaudeModels`,
/// `ResetClaudeBedrock`, the refresh, the switch).
pub trait AccountActions: Send + Sync {
    fn change(&self, provider: &str);
    fn logout(&self, provider: &str);
    fn sign_in(&self, provider: &str, option: CliLoginOption);
    fn cancel_wait(&self, provider: &str);
    fn reset_models(&self);
    fn reset_bedrock(&self);
    fn refresh(&self, provider: &str);
    fn auto_login(&self, on: bool);
}

/// `Place`: a part of the account's line in a column of its own, 8 after the one before.
fn place(row: &mut Vec<El>, cols: &mut Vec<Track>, part: El, width: Track) -> usize {
    let first = cols.is_empty();
    cols.push(width);
    let col = cols.len() as i64 - 1;
    let mut part = part.cell(0, col);
    if !first {
        part.margin = Thickness::new(ACCOUNT_GAP, part.margin.top, 0.0, 0.0);
    }
    row.push(part);
    row.len() - 1
}

/// `AccountRow`: the provider's mark, its name (with the 베타 capsule) over the status line, the detail, the pending line and the
/// row's message, then the ring and the buttons the state allows, Claude's model reset and Bedrock buttons, the refresh mark, and
/// Claude's Bedrock reset on the line under them.
pub fn account_row(ui: &mut Ui, row: &AccountRow, reading: bool, actions: &Arc<dyn AccountActions>) -> El {
    let status = &row.status;
    let provider = status.provider.clone();
    let summary = status.summary();
    let (mut parts, mut cols) = (Vec::new(), Vec::new());
    let mark = provider_mark().ha(HAlign::Center).va(VAlign::Top).margin(Thickness::new(0.0, 1.0, 0.0, 0.0));
    place(&mut parts, &mut cols, mark, Track::Pixel(18.0));

    let mut name = vec![ui.settings_text(strings::provider_label(&provider), TextStyle::size(12.0).medium())];
    if providers::is_beta(&provider) {
        name.push(ui.beta_badge());
    }
    let mut words = vec![El::stack(true, metrics::SPACING_SM, name)];
    let ink = if status.logged_in == Some(false) { Token::WaitText } else { Token::Ink2 };
    let mut line =
        ui.settings_text(summary.clone(), TextStyle::size(11.0).ink(ink).selectable()).aid(format!("cli-account-status-{provider}"));
    if let Some(block) = line.text_mut() {
        block.wrapping = Wrapping::NoWrap;
    }
    words.push(line.tooltip(summary.clone()));
    if !status.detail.is_empty() && status.detail != summary {
        words.push(if status.logged_in == Some(true) {
            ui.settings_tertiary(status.detail.clone(), 10.0, false, false)
        } else {
            ui.settings_text(status.detail.clone(), TextStyle::size(10.0).ink(Token::Ink2))
        });
    }
    if row.pending {
        words.push(ui.settings_tertiary(copy::status_pending(), 10.0, false, false));
    }
    if let Some(said) = &row.message {
        let warning = ui.settings_symbol(WARNING_GLYPH, 10.0, Token::WaitText).va(VAlign::Top).margin(Thickness::new(0.0, 1.0, 0.0, 0.0));
        let message = ui.settings_text(said.clone(), TextStyle::size(10.0).ink(Token::WaitText)).cell(0, 1);
        words.push(
            El::grid(Grid {
                children: vec![warning, message],
                cols: vec![Track::Auto, Track::Star(1.0)],
                col_spacing: metrics::SPACING_XS,
                ..Grid::default()
            })
            .aid(format!("cli-account-message-{provider}")),
        );
    }
    place(&mut parts, &mut cols, El::stack(false, metrics::SPACING_XXS, words).ha(HAlign::Left), Track::Star(ACCOUNT_GIVE));
    cols.push(Track::Star(1.0));

    let turning = reading || row.working || status.logged_in.is_none() && status.detail == copy::status_checking();
    let ring = El::new(Kind::ProgressRing { active: turning }).width(16.0).height(16.0).va(VAlign::Top).visible(turning);
    place(&mut parts, &mut cols, ring, Track::Auto);

    // A button of the row waits while the row works (`holds`); "cancel waiting" never does: it is what ends the wait.
    let held = !row.working;
    let push = |ui: &Ui, title: String, aid: Option<String>, action: Box<dyn Fn() + Send + Sync>| {
        let mut button = ui.settings_push(ui.button(title, action), ControlSize::Regular, false, false).va(VAlign::Top);
        if let Some(aid) = aid {
            button.aid = Some(aid);
        }
        button
    };
    if !status.installed {
        let missing = ui.settings_text(copy::status_not_installed(), TextStyle::size(11.0).ink(Token::Ink2)).va(VAlign::Top);
        place(&mut parts, &mut cols, missing, Track::Auto);
    } else if row.pending {
        let (act, p) = (actions.clone(), provider.clone());
        let cancel = push(
            ui,
            copy::button_cancel_wait(),
            Some(format!("cli-account-cancel-wait-{provider}")),
            Box::new(move || act.cancel_wait(&p)),
        );
        place(&mut parts, &mut cols, cancel, Track::Auto);
    } else if status.logged_in == Some(true) {
        if status.can_sign_out {
            let (act, p) = (actions.clone(), provider.clone());
            let change = push(ui, copy::button_change(), Some(format!("cli-account-change-{provider}")), Box::new(move || act.change(&p)))
                .enabled(held);
            place(&mut parts, &mut cols, change, Track::Auto);
            let (act, p) = (actions.clone(), provider.clone());
            let logout = push(ui, copy::button_logout(), None, Box::new(move || act.logout(&p))).enabled(held);
            place(&mut parts, &mut cols, logout, Track::Auto);
        }
    } else if provider == "claude" {
        let menu = [(copy::button_login_claude(), CliLoginOption::Account), (copy::button_login_console(), CliLoginOption::Console)]
            .into_iter()
            .map(|(title, option)| {
                let act = actions.clone();
                MenuItem::item(title, move || act.sign_in("claude", option))
            })
            .collect();
        let login = ui.settings_menu_button(copy::button_login(), menu).aid("cli-account-login-claude").va(VAlign::Top).enabled(held);
        place(&mut parts, &mut cols, login, Track::Auto);
    } else {
        let (act, p) = (actions.clone(), provider.clone());
        let login = push(
            ui,
            copy::button_login(),
            Some(format!("cli-account-login-{provider}")),
            Box::new(move || act.sign_in(&p, CliLoginOption::Account)),
        )
        .enabled(held);
        place(&mut parts, &mut cols, login, Track::Auto);
    }

    let mut reset_bedrock = None;
    if status.installed && provider == "claude" {
        let title = strings::get("settings.cliAccounts.resetModelsButton");
        let act = actions.clone();
        let mut reset = push(ui, title.clone(), Some("cli-account-reset-models-claude".into()), Box::new(move || act.reset_models()))
            .tooltip(strings::get("settings.cliAccounts.resetModelsHelp"))
            .ha(HAlign::Stretch)
            .enabled(held);
        if let Some(button) = reset.button_mut() {
            button.content = Content::El(Box::new(El::text_block(title)));
        }
        place(&mut parts, &mut cols, reset, Track::Star(ACCOUNT_GIVE));
        let act = actions.clone();
        let bedrock = push(
            ui,
            strings::get("settings.cliAccounts.bedrockButton"),
            Some("cli-account-bedrock-claude".into()),
            Box::new(move || act.sign_in("claude", CliLoginOption::Bedrock)),
        )
        .tooltip(strings::get("settings.cliAccounts.bedrockHelp"))
        .enabled(held);
        place(&mut parts, &mut cols, bedrock, Track::Auto);
        // The reset is Windows' only, as C# and the Mac's Swift app build it.
        if mighty_run::os::windows() {
            let act = actions.clone();
            reset_bedrock = Some(
                push(
                    ui,
                    strings::get("settings.cliAccounts.resetBedrockButton"),
                    Some("cli-account-reset-bedrock-claude".into()),
                    Box::new(move || act.reset_bedrock()),
                )
                .enabled(held),
            );
        }
    }
    let (act, p) = (actions.clone(), provider.clone());
    let refresh = ui
        .settings_icon_button(REFRESH_GLYPH, copy::refresh_tooltip(), move || act.refresh(&p), 13.0)
        .aid(format!("cli-account-refresh-{provider}"))
        // C#'s `Held` sets every row button at the line's top.
        .va(VAlign::Top)
        .enabled(held);
    let at = place(&mut parts, &mut cols, refresh, Track::Auto);
    parts[at].margin = Thickness::new(parts[at].margin.left, -1.0, 0.0, 0.0);
    // Windows' own Bedrock reset takes the line under the Mac's buttons, at its right.
    if let Some(reset) = reset_bedrock {
        parts.push(reset.ha(HAlign::Right).cell(1, 0).span(1, cols.len() as i64));
    }
    El::grid(Grid { children: parts, cols, rows: vec![Track::Auto, Track::Auto], row_spacing: metrics::SPACING_SM, ..Grid::default() })
        .aid(format!("{ACCOUNT_ROW_ID_PREFIX}{provider}"))
}

/// A push button of an account (Regular, at the line's top), its automation id if it has one.
fn account_push(ui: &Ui, title: String, aid: Option<String>, prominent: bool, action: impl Fn() + Send + Sync + 'static) -> El {
    let mut button = ui.settings_push(ui.button(title, action), ControlSize::Regular, prominent, false).va(VAlign::Top);
    if let Some(aid) = aid {
        button.aid = Some(aid);
    }
    button
}

/// 구성 요소' sign-in of an agent CLI (`cli-account-login-<provider>`): Claude's a menu of its two accounts, the others' one button;
/// `prominent` when it is the row's most needed action. It waits while the row works.
pub fn login_button(ui: &Ui, row: &AccountRow, actions: &Arc<dyn AccountActions>, prominent: bool) -> El {
    let provider = row.status.provider.clone();
    let held = !row.working;
    if provider == "claude" {
        let menu = [(copy::button_login_claude(), CliLoginOption::Account), (copy::button_login_console(), CliLoginOption::Console)]
            .into_iter()
            .map(|(title, option)| {
                let act = actions.clone();
                MenuItem::item(title, move || act.sign_in("claude", option))
            })
            .collect();
        let title = copy::button_login();
        let button = El::new(Kind::Button(mighty_native::ui::Button {
            content: Content::Text(title.clone()),
            flyout: Some(menu),
            ..Default::default()
        }))
        .named(title)
        .enabled(held);
        return ui.settings_push(button, ControlSize::Regular, prominent, false).va(VAlign::Top).aid("cli-account-login-claude");
    }
    let act = actions.clone();
    account_push(ui, copy::button_login(), Some(format!("cli-account-login-{provider}")), prominent, move || {
        act.sign_in(&provider, CliLoginOption::Account)
    })
    .enabled(held)
}

/// 구성 요소' account buttons of an installed agent CLI, for its opened row: 대기 취소 while a terminal sign-in is waited on, else
/// 계정 변경 and 로그아웃 while signed in (when the app can sign it out), else the sign-in ([`login_button`], left out when
/// `with_login` is None: the row shows it elsewhere); then Claude's 모델 초기화, Bedrock 설정 and Windows' Bedrock 초기화, and 상태
/// 새로고침. None of a CLI that is not installed. Each waits while the row works, but 대기 취소, which ends the wait.
pub fn account_buttons(ui: &Ui, row: &AccountRow, actions: &Arc<dyn AccountActions>, with_login: Option<bool>) -> Vec<El> {
    let status = &row.status;
    let provider = status.provider.clone();
    let held = !row.working;
    let mut buttons = vec![];
    if !status.installed {
        return buttons;
    }
    if row.pending {
        let (act, p) = (actions.clone(), provider.clone());
        buttons.push(account_push(ui, copy::button_cancel_wait(), Some(format!("cli-account-cancel-wait-{provider}")), false, move || {
            act.cancel_wait(&p)
        }));
    } else if status.logged_in == Some(true) {
        if status.can_sign_out {
            let (act, p) = (actions.clone(), provider.clone());
            buttons.push(
                account_push(ui, copy::button_change(), Some(format!("cli-account-change-{provider}")), false, move || act.change(&p))
                    .enabled(held),
            );
            let (act, p) = (actions.clone(), provider.clone());
            buttons.push(
                account_push(ui, copy::button_logout(), Some(format!("cli-account-logout-{provider}")), false, move || act.logout(&p))
                    .enabled(held),
            );
        }
    } else if let Some(prominent) = with_login {
        buttons.push(login_button(ui, row, actions, prominent));
    }
    if provider == "claude" {
        let act = actions.clone();
        buttons.push(
            account_push(
                ui,
                strings::get("settings.cliAccounts.resetModelsButton"),
                Some("cli-account-reset-models-claude".into()),
                false,
                move || act.reset_models(),
            )
            .tooltip(strings::get("settings.cliAccounts.resetModelsHelp"))
            .enabled(held),
        );
        let act = actions.clone();
        buttons.push(
            account_push(
                ui,
                strings::get("settings.cliAccounts.bedrockButton"),
                Some("cli-account-bedrock-claude".into()),
                false,
                move || act.sign_in("claude", CliLoginOption::Bedrock),
            )
            .tooltip(strings::get("settings.cliAccounts.bedrockHelp"))
            .enabled(held),
        );
        // The reset is Windows' only, as C# and the Mac's Swift app build it.
        if mighty_run::os::windows() {
            let act = actions.clone();
            buttons.push(
                account_push(
                    ui,
                    strings::get("settings.cliAccounts.resetBedrockButton"),
                    Some("cli-account-reset-bedrock-claude".into()),
                    false,
                    move || act.reset_bedrock(),
                )
                .enabled(held),
            );
        }
    }
    let (act, p) = (actions.clone(), provider.clone());
    buttons.push(
        account_push(ui, copy::refresh_tooltip(), Some(format!("cli-account-refresh-{provider}")), false, move || act.refresh(&p))
            .enabled(held),
    );
    buttons
}

/// `BuildCliAccountsSection(statuses)`: a row per account, the section's explanation, then the automatic sign-in switch over its
/// explanation (on unless saved off).
pub fn accounts(ui: &mut Ui, model: &AccountsModel, actions: Arc<dyn AccountActions>) -> El {
    let mut rows = ui.settings_rows();
    for row in &model.rows {
        let built = account_row(ui, row, model.reading, &actions);
        rows.add(built);
    }
    rows.add(ui.settings_text(copy::section_description(), TextStyle::size(11.0).ink(Token::Ink2)));
    for row in auto_login_rows(ui, model.auto_login, &actions) {
        rows.add(row);
    }
    rows.finish()
}

/// The automatic sign-in switch (`cli-auto-login`, on unless saved off) over its explanation: two rows of a box.
pub fn auto_login_rows(ui: &Ui, on: bool, actions: &Arc<dyn AccountActions>) -> [El; 2] {
    let (switch, label, help) = auto_login_switch(ui, on, actions);
    [
        ui.settings_labeled(ui.settings_text(label, TextStyle::default()), switch, false),
        ui.settings_text(help, TextStyle::size(11.0).ink(Token::Ink2)),
    ]
}

/// The automatic sign-in switch (`cli-auto-login`) with its label and its explanation.
pub fn auto_login_switch(ui: &Ui, on: bool, actions: &Arc<dyn AccountActions>) -> (El, String, String) {
    let label = strings::get("settings.cliAccounts.autoLoginToggle");
    let act = actions.clone();
    let switch = ui.settings_switch(label.clone(), on, Some("cli-auto-login"), move |on| act.auto_login(on));
    (switch, label, strings::get("settings.cliAccounts.autoLoginDescription"))
}

/// A dialog whose content is one wrapping sentence (C#'s `Content = new TextBlock { Text, TextWrapping = Wrap }`: an alert).
fn sentence(text: String) -> impl FnMut(&mut Ui) -> Content + Send + 'static {
    move |_| {
        let mut block = El::text_block(text.clone());
        if let Some(words) = block.text_mut() {
            words.wrapping = Wrapping::Wrap;
        }
        Content::El(Box::new(block))
    }
}

/// D8, `ConfirmCliSignOut`'s dialog: the stored sign-in of that CLI is removed (also for the CLI used in a terminal); "계정 변경"
/// (`relogin`) asks the same under its own title and button.
pub fn sign_out_dialog(provider: &str, relogin: bool) -> DialogSpec {
    let label = strings::provider_label(provider);
    let title = if relogin { copy::confirm_change_title_template() } else { copy::confirm_logout_title_template() };
    let mut spec =
        DialogSpec::new(title.replace("{provider}", label), sentence(copy::confirm_message_template().replace("{provider}", label)))
            .styled(None, None);
    let primary = if relogin { strings::get("settings.cliAccounts.macReloginButton") } else { copy::button_logout() };
    spec.primary = Some(DialogButton::new(primary));
    spec.close = Some(copy::button_cancel());
    spec
}

/// D9, `ResetClaudeBedrock`'s dialog.
pub fn reset_bedrock_dialog() -> DialogSpec {
    let title = strings::get("settings.cliAccounts.resetBedrockButton");
    let mut spec = DialogSpec::new(title.clone(), sentence(strings::get("settings.cliAccounts.resetBedrockConfirm"))).styled(None, None);
    spec.primary = Some(DialogButton::new(title));
    spec.close = Some(copy::button_cancel());
    spec
}
