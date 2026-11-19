//! `cli`: CLI 계정 (C# MainWindow.Settings.cs:573-1026). Owner: W6-C.
//!
//! The box is mighty-accounts' builder over the host's models (accounts.rs): the accounts' statuses and their rows' state. The tab's
//! old 이 PC의 CLI and Claude Mods boxes are gone since the Settings tidy-up: 구성 요소 lists each CLI with its state, version,
//! install, sign-in and update, and says when Claude Code is too old for Claude Mods, so the CLI tab keeps the accounts and updates. The CLI accounts box shows the smoke's fixture statuses while the accounts step holds them (C#'s smoke hands
//! `BuildCliAccountsSection` its fixture statuses).

use std::sync::{Arc, Mutex};

use mighty_accounts::accounts::CliAccountStatus;
use mighty_native::ui::{El, Ui};

use crate::host::Host;

/// The smoke's fixture statuses while its accounts step shows them (C# `CliAccountSmoke.FixtureStatuses`).
static FIXTURE: Mutex<Option<Vec<CliAccountStatus>>> = Mutex::new(None);

/// The smoke shows (Some) or stops showing (None) its fixture statuses in the box.
pub fn show_fixture(statuses: Option<Vec<CliAccountStatus>>) {
    *FIXTURE.lock().unwrap_or_else(|p| p.into_inner()) = statuses;
}

/// CLI 계정 (CLI accounts) (C# `BuildCliAccountsSectionFromState`, MainWindow.Settings.cs:628-1026).
pub fn accounts(host: &Arc<Host>, ui: &mut Ui) -> El {
    let fixture = FIXTURE.lock().unwrap_or_else(|p| p.into_inner()).clone();
    let model = crate::accounts::accounts_model(host, fixture);
    mighty_accounts::view::accounts(ui, &model, Arc::new(crate::accounts::TabActions { host: host.clone() }))
}
