//! Delegation's git (C# Core/DelegationWorktree.cs, DelegationMerge.cs, DelegationCleanup.cs, `ChildAutoCommit`, `ChildReportFile`,
//! `DelegationPaths`; the fix branch `fix/csharp-from-tauri-review` for the runner's settings and `RemoveOwnWorktree`), every command
//! through mighty-run's [`mighty_run::git`] runner, byte for byte as W7 plan section 1.7 lists them. Owner: W7-B.
//!
//! The seams the other parts call: `worktree::{check, make, is_safe_session_id, worktree_path}`, `merge::{tool_merge, card_merge,
//! undo, follow_branch, head, has_tracked_changes}`, `cleanup::{clean_up, discard, remove_own_worktree, nested_worktrees}`,
//! `auto_commit::run`, `report_file::read`, `paths::{canonical, same}`.

pub mod auto_commit;
pub mod cleanup;
pub mod merge;
pub mod paths;
pub mod report_file;
pub mod worktree;
