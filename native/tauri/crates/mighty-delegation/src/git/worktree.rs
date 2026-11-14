//! The worktree maker (C# Core/DelegationWorktree.cs:1-172; M/DelegationWorktree.swift): a delegated child's own checkout, the flat
//! branch `mighty/<session id>` at its parent's head, in a git worktree under the worktree root, outside the workspace checkout.
//! TASK.md and REPORT.md sit in [`NOTES_FOLDER`] inside it; that folder ignores itself, so neither file is ever committed or counts as a
//! change, whatever line endings git is set to write. The check (inside a work tree, on a branch, born, 10,000,000,000 bytes free
//! through mighty-platform's `DiskSpace`) comes first and makes nothing. Owner: W7-B.

use std::io::Write;
use std::path::Path;
use std::sync::Arc;

use mighty_core::normalize::fully_qualified;
use mighty_platform::DiskSpace;
use mighty_run::os;

use super::paths::standardized;
use super::{Git, branch_name, is_object_name, line, message, run};
use crate::request::DelegationReasonCode;

/// `ChildWorktree.NotesFolder`.
pub const NOTES_FOLDER: &str = ".mighty-delegation";

/// `ChildWorktreeMaker.MinimumFreeBytes`.
pub const MINIMUM_FREE_BYTES: u64 = 10_000_000_000;

/// `ChildWorktree.TaskFileIn`: `<worktree>\.mighty-delegation\TASK.md`.
pub fn task_file_in(worktree_path: &str) -> String {
    os::combine(&os::combine(worktree_path, NOTES_FOLDER), "TASK.md")
}

/// `ChildWorktree.ReportFileIn`: `<worktree>\.mighty-delegation\REPORT.md`.
pub fn report_file_in(worktree_path: &str) -> String {
    os::combine(&os::combine(worktree_path, NOTES_FOLDER), "REPORT.md")
}

/// `ChildWorktreeMaker.DefaultRoot`: `%USERPROFILE%\.mightycode\worktrees`, given the user's profile folder (the host reads it from
/// its `Paths` port; a check never names the real one).
pub fn default_root(user_profile: &str) -> String {
    os::combine(&os::combine(user_profile, ".mightycode"), "worktrees")
}

/// `ChildWorktree`: where the child works (`working_folder`: the worktree, or the same subfolder of it when the workspace is a
/// subfolder of its repository), its branch, and the parent's branch with its head (`base_commit`) when the child was made.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChildWorktree {
    pub session_id: String,
    pub worktree_path: String,
    pub working_folder: String,
    pub branch: String,
    pub parent_branch: String,
    pub base_commit: String,
}

impl ChildWorktree {
    pub fn task_file(&self) -> String {
        task_file_in(&self.worktree_path)
    }

    pub fn report_file(&self) -> String {
        report_file_in(&self.worktree_path)
    }
}

/// `ChildWorktreeBase`: the parent side of a child about to be made, read from the workspace. `repository`: the top folder of the
/// workspace's checkout; `prefix`: the workspace's folder inside it, as git names it (`""` or `"sub/folder/"`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChildWorktreeBase {
    pub repository: String,
    pub prefix: String,
    pub parent_branch: String,
    pub base_commit: String,
}

/// `ChildWorktreeCheck`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChildWorktreeCheck {
    Ready(ChildWorktreeBase),
    /// Refused before anything was made.
    Refused(DelegationReasonCode),
}

impl ChildWorktreeCheck {
    /// `CanHostWorktree`: a repository with a branch checked out that has a commit. Free disk changes from minute to minute, so a low
    /// disk still counts as one; the child's own check refuses it then.
    pub fn can_host_worktree(&self) -> bool {
        matches!(self, ChildWorktreeCheck::Ready(_) | ChildWorktreeCheck::Refused(DelegationReasonCode::LowDisk))
    }
}

/// `ChildWorktreeOutcome`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChildWorktreeOutcome {
    Created(ChildWorktree),
    /// Refused before anything was made.
    Refused(DelegationReasonCode),
    /// Git or the file system failed partway. Whatever was left (a branch, a folder) stays until a human discards the child; the app
    /// never removes it on its own.
    Failed(String),
}

/// `ChildWorktreeMaker`: makes child worktrees under one root (`%USERPROFILE%\.mightycode\worktrees` in the app, a temp folder in
/// tests). A workspace that is not in a git checkout, whose branch has no commit yet, whose HEAD is detached, or whose root volume has
/// under 10 GB free is refused before anything is made.
#[derive(Clone)]
pub struct Maker {
    root: String,
    git: Git,
    disk: Arc<dyn DiskSpace>,
}

impl Maker {
    /// `disk` reads the free space of the volume holding the root; an error (unknown) counts as too little.
    pub fn new(root: &str, git: Git, disk: Arc<dyn DiskSpace>) -> Maker {
        Maker { root: standardized(root), git, disk }
    }

    /// The root, standardized.
    pub fn root(&self) -> &str {
        &self.root
    }

    pub fn git(&self) -> &Git {
        &self.git
    }

    /// `WorktreePath`: `<root>\<session id>`.
    pub fn worktree_path(&self, session_id: &str) -> String {
        os::combine(&self.root, session_id)
    }

    /// `Check`: the workspace and the free space, making nothing. Quick, so a delegate call can answer with its refusal.
    pub async fn check(&self, workspace: &str) -> ChildWorktreeCheck {
        use ChildWorktreeCheck::Refused;
        if !fully_qualified(workspace) || workspace.contains('\0') {
            return Refused(DelegationReasonCode::NotGit);
        }
        let Some(place) = run(&self.git, &["rev-parse", "--is-inside-work-tree", "--show-toplevel", "--show-prefix"], workspace, 10.0)
            .await
            .filter(|place| place.exit_code == 0)
        else {
            return Refused(DelegationReasonCode::NotGit);
        };
        let lines: Vec<&str> = place.stdout.split('\n').map(|line| line.trim_end_matches('\r')).collect();
        if lines.len() < 2 || lines[0] != "true" || !fully_qualified(lines[1]) {
            return Refused(DelegationReasonCode::NotGit);
        }
        let Some(head) = run(&self.git, &["symbolic-ref", "-q", "HEAD"], workspace, 10.0).await else {
            return Refused(DelegationReasonCode::NotGit);
        };
        let reference = line(&head.stdout);
        if head.exit_code != 0 || !reference.starts_with("refs/heads/") || reference.len() <= 11 {
            return Refused(if matches!(head.exit_code, 0 | 1) {
                DelegationReasonCode::DetachedHead
            } else {
                DelegationReasonCode::NotGit
            });
        }
        let Some(commit) = run(&self.git, &["rev-parse", "-q", "--verify", "HEAD^{commit}"], workspace, 10.0).await else {
            return Refused(DelegationReasonCode::NotGit);
        };
        let sha = line(&commit.stdout);
        if commit.exit_code != 0 || !is_object_name(sha) {
            return Refused(DelegationReasonCode::UnbornBranch);
        }
        if !self.disk.available_bytes(Path::new(&self.root)).is_ok_and(|free| free >= MINIMUM_FREE_BYTES) {
            return Refused(DelegationReasonCode::LowDisk);
        }
        // Git names the folder with forward slashes; the rest of the app reads it the Windows way.
        ChildWorktreeCheck::Ready(ChildWorktreeBase {
            repository: standardized(lines[1]),
            prefix: lines.get(2).copied().unwrap_or("").to_string(),
            parent_branch: reference[11..].to_string(),
            base_commit: sha.to_string(),
        })
    }

    /// `Make`: the branch `mighty/<session id>` at the recorded base commit and its worktree (`worktree add -b`), then TASK.md with
    /// `task` and an empty REPORT.md. Never replaces an existing branch, folder or file. `timeout` in seconds (C#'s default is 300).
    pub async fn make(&self, session_id: &str, start: &ChildWorktreeBase, task: &str, timeout: f64) -> ChildWorktreeOutcome {
        use ChildWorktreeOutcome::Failed;
        if !is_safe_session_id(session_id) {
            return Failed("The session id cannot name a branch and a folder.".into());
        }
        let worktree = self.worktree_path(session_id);
        let branch = branch_name(session_id);
        if let Err(error) = std::fs::create_dir_all(&self.root) {
            return Failed(error.to_string());
        }
        let Some(added) =
            run(&self.git, &["worktree", "add", "-b", &branch, &worktree, &start.base_commit], &start.repository, timeout).await
        else {
            return Failed("Git did not finish.".into());
        };
        if added.exit_code != 0 {
            return Failed(message(&added));
        }
        if let Err(error) = write_notes(&worktree, task) {
            return Failed(error.to_string());
        }
        let folder = if start.prefix.is_empty() { worktree.clone() } else { standardized(&os::combine(&worktree, &start.prefix)) };
        ChildWorktreeOutcome::Created(ChildWorktree {
            session_id: session_id.to_string(),
            worktree_path: worktree,
            working_folder: folder,
            branch,
            parent_branch: start.parent_branch.clone(),
            base_commit: start.base_commit.clone(),
        })
    }

    /// `Create`: [`Maker::check`] and then [`Maker::make`] with C#'s 300 s.
    pub async fn create(&self, session_id: &str, workspace: &str, task: &str) -> ChildWorktreeOutcome {
        match self.check(workspace).await {
            ChildWorktreeCheck::Ready(start) => self.make(session_id, &start, task, 300.0).await,
            ChildWorktreeCheck::Refused(reason) => ChildWorktreeOutcome::Refused(reason),
        }
    }
}

/// The notes folder with its `.gitignore` (`*\n`), TASK.md (`task` as UTF-8, no BOM) and an empty REPORT.md, each created new.
/// Written byte for byte, never through git: the notes ignore themselves, so no line-ending setting touches them.
fn write_notes(worktree: &str, task: &str) -> std::io::Result<()> {
    let notes = os::combine(worktree, NOTES_FOLDER);
    if Path::new(&notes).exists() {
        return Err(std::io::Error::new(std::io::ErrorKind::AlreadyExists, notes));
    }
    std::fs::create_dir_all(&notes)?;
    for (path, bytes) in [
        (os::combine(&notes, ".gitignore"), b"*\n".as_slice()),
        (task_file_in(worktree), task.as_bytes()),
        (report_file_in(worktree), b"".as_slice()),
    ] {
        std::fs::OpenOptions::new().write(true).create_new(true).open(&path)?.write_all(bytes)?;
    }
    Ok(())
}

/// `IsSafeSessionId`: letters, digits, `-`, `_` and inner dots, at most 128: one folder name and a valid branch name part. On Windows a
/// device name (`nul`, `con`, `com1`…) is no folder name either, with or without an extension.
pub fn is_safe_session_id(id: &str) -> bool {
    let shaped = (1..=128).contains(&id.len())
        && !id.starts_with(['.', '-'])
        && !id.ends_with('.')
        && !id.contains("..")
        && !id.ends_with(".lock")
        && id.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '.' | '_'));
    shaped && !(os::windows() && is_device_name(id.split('.').next().unwrap_or("")))
}

/// `^(con|prn|aux|nul|com[1-9]|lpt[1-9])$`, ignoring case.
fn is_device_name(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    matches!(name.as_str(), "con" | "prn" | "aux" | "nul")
        || (name.len() == 4 && (name.starts_with("com") || name.starts_with("lpt")) && matches!(name.as_bytes()[3], b'1'..=b'9'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_session_id_is_one_folder_and_one_branch_part() {
        let mut unsafe_ids =
            vec!["", "../escape", "a/b", r"a\b", ".hidden", "-flag", "x.lock", "a..b", "sp ace", "\u{D55C}\u{AE00}", "a:b", "trailing."];
        if os::windows() {
            unsafe_ids.extend(["nul", "CON", "com1.txt", "LPT9"]);
        }
        for id in unsafe_ids {
            assert!(!is_safe_session_id(id), "{id}");
        }
        let long = "a".repeat(129);
        assert!(!is_safe_session_id(&long) && is_safe_session_id(&long[..128]));
        for id in [
            mighty_core::wire::new_id(),
            uuid::Uuid::new_v4().to_string().to_uppercase(),
            "console".into(),
            "a.b_c-d".into(),
            "com0".into(),
        ] {
            assert!(is_safe_session_id(&id), "{id}");
        }
    }
}
