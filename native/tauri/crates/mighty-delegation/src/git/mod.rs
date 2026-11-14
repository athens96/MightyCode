//! Delegation's git (C# Core/DelegationWorktree.cs, DelegationMerge.cs, DelegationCleanup.cs, `ChildAutoCommit`, `ChildReportFile`,
//! `DelegationPaths`; the fix branch `fix/csharp-from-tauri-review` for the runner's settings and `RemoveOwnWorktree`), every command
//! through mighty-run's [`mighty_run::git`] runner ([`Git::run_detailed`]), byte for byte as W7 plan section 1.7 lists them. Owner:
//! W7-B.
//!
//! The seams the other parts call:
//! - [`worktree::Maker`] (`check`, `make`, `create`, `worktree_path`), [`worktree::is_safe_session_id`], [`worktree::default_root`];
//! - [`merge::tool_merge`], [`merge::card_merge`], [`merge::undo`], [`merge::merge_branch`] (the branch follow), [`merge::head`],
//!   [`merge::checked_out_head`], [`merge::has_tracked_changes`], [`merge::branch_head`], [`merge::conflict_prompt`];
//! - [`cleanup::clean_up`], [`cleanup::discard`], [`cleanup::remove_own_worktree`], [`cleanup::nested_worktrees`],
//!   [`cleanup::workspace_removal_refusal`];
//! - [`auto_commit::run`], [`report_file::read`], [`paths::DelegationPaths`] (`canonical`), [`paths::same`], [`paths::is_inside`],
//!   [`paths::standardized`].
//!
//! What the git layer reads of a child is [`ChildGit`], borrowed from W7-A's `ChildRecord`, so this layer names no record of its own.

use std::path::Path;

pub use mighty_run::git::{Git, GitFailure, GitResult, GitRun, is_object_name, line};

use crate::model::ChildState;

pub mod auto_commit;
pub mod cleanup;
pub mod merge;
pub mod paths;
pub mod report_file;
pub mod worktree;

/// `ChildRecord.BranchName`: always `mighty/<session id>`, flat.
pub fn branch_name(session_id: &str) -> String {
    format!("mighty/{session_id}")
}

/// What delegation's git reads of a child (C# `ChildRecord`'s members of the same names): `branch` is `mighty/<id>`, `parent_branch`
/// and `base_commit` the parent's branch and its head when the child was made, `report_head` the head of its last report,
/// `parent_checkout` the parent's folder recorded when it was made.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChildGit<'a> {
    pub id: &'a str,
    pub parent_session_id: &'a str,
    pub worktree_path: &'a str,
    pub branch: &'a str,
    pub parent_branch: &'a str,
    pub base_commit: &'a str,
    pub state: ChildState,
    pub report_head: Option<&'a str>,
    pub parent_checkout: Option<&'a str>,
}

/// `DelegationGit.Run`: [`Git::run_detailed`] without input, its result only when nothing failed. `timeout` in seconds (C#'s default
/// is 10).
pub async fn run(git: &Git, arguments: &[&str], directory: &str, timeout: f64) -> Option<GitResult> {
    git.run_detailed(arguments, Path::new(directory), timeout, None).await.ok()
}

/// `DelegationGit.Message`: git's own error text, at most 500 text elements, or `Git exited with <code>.` when it wrote none.
pub fn message(result: &GitResult) -> String {
    let text = line(&result.stderr);
    if text.is_empty() { format!("Git exited with {}.", result.exit_code) } else { crate::request::prefix(text, 500).to_string() }
}

/// `Directory.Exists`.
pub(crate) fn is_folder(path: &str) -> bool {
    Path::new(path).is_dir()
}

/// `Directory.Exists(path) || File.Exists(path)`.
pub(crate) fn exists(path: &str) -> bool {
    Path::new(path).exists()
}

/// `Path.IsPathFullyQualified(folder) && !folder.Contains('\0') && Directory.Exists(folder)`: a folder git may be pointed at.
pub(crate) fn usable_folder(folder: &str) -> bool {
    mighty_core::normalize::fully_qualified(folder) && !folder.contains('\0') && is_folder(folder)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_failure_carries_gits_words_or_its_exit_code() {
        let result = |exit_code: i32, stderr: &str| GitResult { exit_code, stdout: "out".into(), stderr: stderr.into() };
        assert_eq!(message(&result(128, "  fatal: no\n")), "fatal: no");
        assert_eq!(message(&result(2, " ")), "Git exited with 2.");
        assert_eq!(message(&result(1, &"x".repeat(900))).len(), 500);
        assert_eq!(branch_name("c1"), "mighty/c1");
        assert!(is_object_name(&"a".repeat(40)) && is_object_name(&"F".repeat(64)));
        assert!(!is_object_name(&"a".repeat(39)) && !is_object_name(&"g".repeat(40)) && !is_object_name("HEAD"));
    }
}
