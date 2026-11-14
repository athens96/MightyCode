//! The smoke's delegation step (C# WinUI/MainWindow.DelegationSmoke.cs; live key `delegation`, `delegationError` on failure): after the
//! ACP step, before the run steps. Owner: W7-D.
//!
//! What W7-D fills here (W7 plan, W7-D "Smoke"): the fixture `<profile>\Delegation Smoke\{repo, fake}` (`git init -b main`, a README
//! committed as `Mighty Smoke <smoke@example.invalid>`), Claude overridden to the app's own exe `--fake-claude <fake>`, the switch in
//! memory, worktrees under `<profile>\worktrees` with unlimited free space, three reported children, a pending notice and a restart;
//! the keys `childrenListed`, `sidebarTree`, `heldRowPinned`, `windowCloseMergedChild`, `phoneCloseMergedChild`,
//! `phoneSendReleasedHeldNotice`, `heldNoticeReceipt`, `secondSendNewWorktree`, `removeRefusedOpenChild`, `windowParentCloseAsks`,
//! `phoneParentCloseKeepsChild`, `removeRefusedKeptWorktree`, `removedAfterDiscard`, `otherAgentChildStarted`, `passed`, and the
//! Rust-only `delegateTool` (the fake Claude's delegate call through the real exe mode and pipe). Every git step targets the fixture
//! under the smoke's profile (`Host::may_touch_repo`); nothing reaches `%USERPROFILE%\.mightycode`.

use std::sync::Arc;

use serde_json::Value;

use crate::host::Host;

/// Starts the step; false when it does not run (then the smoke goes on to the next one). At rest: it does not run.
pub fn start(host: &Arc<Host>) -> bool {
    let _ = host;
    false
}

/// The step's checks from its report. None at rest (a step that never started adds none).
pub fn checks(report: &Value) -> Vec<(&'static str, bool)> {
    let _ = report;
    vec![]
}
