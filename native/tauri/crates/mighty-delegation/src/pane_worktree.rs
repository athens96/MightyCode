//! Per-pane git worktrees (C# `Core\PaneWorktree.cs`, 632 lines, from `main` 0.4.60–0.4.61; the Mac's `PaneWorktree*.swift` as the
//! tie-breaker; plan §3 W9-I). Owner: W9-I.
//!
//! A workspace's `paneWorktreeMode` gives each agent pane a worktree of its own (`worktreeSetupCommand`, `worktreeCopyPaths`,
//! `keptPaneWorktrees`; `RunSession.paneWorktree*`), made, merged, committed and merged, resolved, undone, turned into a PR, discarded,
//! listed and diffed over the W7 git layer ([`crate::git`]: `remove_own_worktree`, `place.rs`), writes only where `may_touch_repo`
//! allows (W7). The host's methods (`paneWorktreeStatus`, `paneWorktreeAction`, `paneWorktreeDiff`, `workspaceWorktree`) are src-tauri's
//! `pane_worktree.rs`.
//!
//! Step 0: empty; W9-I writes it.
