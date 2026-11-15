//! Per-pane worktrees in the host (C# `WinUI\MainWindow.PaneWorktree.cs`, 551 lines, from `main` 0.4.60–0.4.61, over mighty-delegation's
//! `pane_worktree`; the page's `web\pane\{WorktreeBar.tsx,worktree.ts}` and `dialogs\Dialogs.tsx`; plan §3 W9-I). Owner: W9-I.
//!
//! The page methods `paneWorktreeStatus`, `paneWorktreeAction{merge, commitMerge, resolve, undo, pr, discard, changes, terminal, reveal,
//! port}`, `paneWorktreeDiff` and `workspaceWorktree{mode, setup, copy, list}`; the dialog requests `paneWorktreeChoice`,
//! `paneWorktreeText`, `paneWorktreeChanges`, `paneWorktreeList`; `PaneInfo.worktree` and the `paneWorktree.marker` word; the phone's
//! create. Writes follow `Host::may_touch_repo` (W7).

use std::sync::Arc;

use serde_json::Value;

use crate::host::{Host, Registry};

/// The page methods this part answers (see the module comment). Step 0: none registered, so the page's four stay on the unimplemented
/// list (README, the smoke's `ALLOWED_UNIMPLEMENTED`) until W9-I.
pub fn register(_methods: &mut Registry) {}

/// After the run engine (main.rs). Step 0 stub (W9-I writes it): nothing starts.
pub fn start(_host: &Arc<Host>) {}

/// The smoke's pane-worktree step (smoke.rs `Stage::PaneWorktree`, after delegation): true when it started; it reports once through
/// `Smoke::probed`. Step 0 stub (W9-I writes it): it does not start.
pub fn smoke_start(_host: &Arc<Host>) -> bool {
    false
}

/// The step's checks from its report. Step 0 stub (W9-I writes it): none.
pub fn smoke_checks(_report: &Value) -> Vec<(&'static str, bool)> {
    Vec::new()
}
