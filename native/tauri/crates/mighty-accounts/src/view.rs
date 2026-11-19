//! The CLI 계정 box and dialogs D8 and D9 (C# MainWindow.Settings.cs:628-810 `BuildCliAccountsSection`, :905-932 `ConfirmCliSignOut`,
//! :1015-1026 `ResetClaudeBedrock`), built with mighty-native's settings kit from models the host keeps: no I/O here, they run on every
//! read of the settings surface. What a button does is the host's ([`AccountActions`]). C#'s 이 PC의 CLI and Claude Mods boxes are not
//! drawn since the Settings tidy-up: 구성 요소' setup list shows each CLI's state, version and the Claude Code that Mods needs.

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

/// `BuildCliAccountsSection(statuses)`: a row per account, the section's explanation, then the automatic sign-in switch over its
/// explanation (on unless saved off).
pub fn accounts(ui: &mut Ui, model: &AccountsModel, actions: Arc<dyn AccountActions>) -> El {
    let mut rows = ui.settings_rows();
    for row in &model.rows {
        let built = account_row(ui, row, model.reading, &actions);
        rows.add(built);
    }
    rows.add(ui.settings_text(copy::section_description(), TextStyle::size(11.0).ink(Token::Ink2)));
    let label = strings::get("settings.cliAccounts.autoLoginToggle");
    let act = actions.clone();
    let switch = ui.settings_switch(label.clone(), model.auto_login, Some("cli-auto-login"), move |on| act.auto_login(on));
    let labeled = ui.settings_labeled(ui.settings_text(label, TextStyle::default()), switch, false);
    rows.add(labeled);
    rows.add(ui.settings_text(strings::get("settings.cliAccounts.autoLoginDescription"), TextStyle::size(11.0).ink(Token::Ink2)));
    rows.finish()
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
