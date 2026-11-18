//! The app's commit of a Codex or Antigravity child's work (C# Core/DelegationProviders.cs:229-347, `ChildAutoCommit`;
//! M/DelegationProviders.swift): when a run of the child finishes normally with REPORT.md changed and the worktree changed, what it
//! changed is committed on the child's branch before its head is read for the report. Tracked files go in as changed; new files go in
//! as .gitignore lets them (the notes folder ignores itself, so TASK.md and REPORT.md never go in), except new files that look like
//! secrets or are over [`MAXIMUM_NEW_FILE_BYTES`], which stay uncommitted and are named to the human. Never with an unresolved
//! conflict, never in a folder that is not the child's own worktree, and only its own `index.lock` is ever removed. Owner: W7-B.

use std::path::Path;

use mighty_run::os;

use super::paths::{DelegationPaths, is_link, same};
use super::{Git, GitFailure, message};

/// `ChildAutoCommit.MaximumNewFileBytes`: the largest new file the app commits for a child.
pub const MAXIMUM_NEW_FILE_BYTES: u64 = 5_000_000;
/// The longest one git step of the commit may take, in seconds.
const TIMEOUT: f64 = 300.0;

/// `ShouldCommit`: whether the app commits for the child now. A Claude child commits itself, so never; any other child whose report
/// changed, with changes left and no unmerged path.
pub fn should_commit(provider: &str, report_changed: bool, has_changes: bool, unmerged: bool) -> bool {
    provider != "claude" && report_changed && has_changes && !unmerged
}

/// `Message`: the message of the commit for report revision `revision`.
pub fn message_for(child_id: &str, revision: i32) -> String {
    format!("mighty: {child_id} report r{revision}")
}

/// The name starts that look like a secret.
const SECRET_STARTS: [&str; 7] = [".env", "id_rsa", "id_ed25519", "id_ecdsa", "id_dsa", "credentials", "secrets."];
/// The name ends that look like a secret.
const SECRET_ENDS: [&str; 5] = [".pem", ".key", ".pfx", ".p12", ".ppk"];
/// The whole names that look like a secret.
const SECRET_NAMES: [&str; 2] = [".npmrc", ".pypirc"];

/// `LooksSecret`: whether a new file at `path` (relative to the worktree) looks like a secret: `.env*`, `*.pem`, `*.key`, `*.pfx`,
/// `*.p12`, `*.ppk`, `id_rsa*`, `id_ed25519*`, `id_ecdsa*`, `id_dsa*`, `credentials*`, `secrets.*`, `.npmrc` or `.pypirc`, in any case.
pub fn looks_secret(path: &str) -> bool {
    let name = path.replace('\\', "/");
    let name = to_lower_invariant(name.trim_end_matches('/').rsplit('/').next().unwrap_or(""));
    SECRET_STARTS.iter().any(|start| name.starts_with(start))
        || SECRET_ENDS.iter().any(|end| name.ends_with(end))
        || SECRET_NAMES.contains(&name.as_str())
}

/// `ToLowerInvariant`: each character by its simple lowercase mapping, one character for one (`İ` U+0130 is `i`, never `i` and a dot
/// above as Rust's full mapping writes it).
fn to_lower_invariant(text: &str) -> String {
    text.chars()
        .map(|c| match c {
            '\u{130}' => 'i',
            c => {
                let mut lower = c.to_lowercase();
                match (lower.next(), lower.next()) {
                    (Some(single), None) => single,
                    _ => c,
                }
            }
        })
        .collect()
}

/// `ChildAutoCommit.Outcome`: what the app found or did in the child's worktree.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AutoCommitOutcome {
    /// Nothing to commit, or the child is one that commits itself.
    Nothing,
    Committed,
    /// `git ls-files -u` lists paths: a conflict is unresolved, so nothing was committed.
    Unmerged,
    /// The run stopped or errored: nothing is committed until a run of the child finishes normally (the caller's answer; [`run`] never
    /// gives it).
    AfterNormalFinish,
    /// Git could not read or commit the worktree, or it is not the child's own ([`AutoCommitResult::why`]). Nothing was committed.
    Failed,
}

/// `ChildAutoCommit.Result`: `left_out` are the new files left uncommitted (secret-looking or too large).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AutoCommitResult {
    pub outcome: AutoCommitOutcome,
    pub left_out: Vec<String>,
    pub why: Option<String>,
}

impl AutoCommitResult {
    /// `Result.Of`.
    pub fn of(outcome: AutoCommitOutcome) -> AutoCommitResult {
        AutoCommitResult { outcome, left_out: vec![], why: None }
    }

    /// `Result.Failing`.
    pub fn failing(why: impl Into<String>, left_out: Vec<String>) -> AutoCommitResult {
        AutoCommitResult { outcome: AutoCommitOutcome::Failed, left_out, why: Some(why.into()) }
    }
}

/// `ChildAutoCommit.Run`: commits the worktree at `worktree_path` for report revision `revision` when [`should_commit`] says so; the
/// report is known to have changed. A lock file left by a git of the app's own that did not finish in time is removed; one there
/// before stops the commit.
pub async fn run(
    git: &Git,
    paths: &DelegationPaths,
    worktree_path: &str,
    child_id: &str,
    revision: i32,
    provider: &str,
) -> AutoCommitResult {
    use AutoCommitOutcome::{Committed, Nothing, Unmerged};
    if provider == "claude" {
        return AutoCommitResult::of(Nothing);
    }
    let unreadable = || "Git could not read the worktree.";
    let not_own = "The child's folder is not its own git worktree.";
    // A worktree folder replaced by a link or junction is never committed through (beyond C#): git would commit in what it points to.
    if is_link(worktree_path) {
        return AutoCommitResult::failing(not_own, vec![]);
    }
    let Some(top) = super::run(git, &["rev-parse", "--show-toplevel", "--absolute-git-dir"], worktree_path, 10.0)
        .await
        .filter(|top| top.exit_code == 0)
    else {
        return AutoCommitResult::failing(unreadable(), vec![]);
    };
    let lines: Vec<&str> = top.stdout.split('\n').map(str::trim).filter(|line| !line.is_empty()).collect();
    // The worktree's own last name as written, never resolved: git's top folder is the real one.
    if lines.len() != 2 || !same(&paths.canonical(lines[0]), &paths.canonical_place(worktree_path)) {
        return AutoCommitResult::failing(not_own, vec![]);
    }
    let lock_file = os::combine(lines[1], "index.lock");
    let unmerged = super::run(git, &["ls-files", "-u"], worktree_path, TIMEOUT).await.filter(|r| r.exit_code == 0);
    let status = match unmerged {
        Some(_) => super::run(git, &["status", "--porcelain", "--untracked-files=normal"], worktree_path, TIMEOUT)
            .await
            .filter(|r| r.exit_code == 0),
        None => None,
    };
    let (Some(unmerged), Some(status)) = (unmerged, status) else {
        return AutoCommitResult::failing(unreadable(), vec![]);
    };
    if !unmerged.stdout.is_empty() {
        return AutoCommitResult::of(Unmerged);
    }
    if !should_commit(provider, true, !status.stdout.is_empty(), false) {
        return AutoCommitResult::of(Nothing);
    }
    if Path::new(&lock_file).exists() {
        return AutoCommitResult::failing("Another git is working in the worktree (index.lock).", vec![]);
    }
    let Some(others) =
        super::run(git, &["ls-files", "-z", "--others", "--exclude-standard"], worktree_path, TIMEOUT).await.filter(|r| r.exit_code == 0)
    else {
        return AutoCommitResult::failing(unreadable(), vec![]);
    };
    let fresh: Vec<&str> = others.stdout.split('\0').filter(|path| !path.is_empty()).collect();
    let left_out: Vec<String> = fresh
        .iter()
        .filter(|path| looks_secret(path) || size(&os::combine(worktree_path, path)) > MAXIMUM_NEW_FILE_BYTES)
        .map(|path| path.to_string())
        .collect();
    let taken: Vec<&str> = fresh.iter().copied().filter(|path| !left_out.iter().any(|out| out == path)).collect();
    if let Some(failed) = write_step(git, worktree_path, &lock_file, &["add", "-u"], None, TIMEOUT).await {
        return AutoCommitResult::failing(failed, left_out);
    }
    if !taken.is_empty() {
        let list = taken.join("\0");
        let arguments = ["--literal-pathspecs", "add", "--pathspec-from-file=-", "--pathspec-file-nul"];
        if let Some(failed) = write_step(git, worktree_path, &lock_file, &arguments, Some(&list), TIMEOUT).await {
            return AutoCommitResult::failing(failed, left_out);
        }
    }
    let Some(staged) =
        super::run(git, &["diff", "--cached", "--quiet"], worktree_path, TIMEOUT).await.filter(|r| matches!(r.exit_code, 0 | 1))
    else {
        return AutoCommitResult::failing(unreadable(), left_out);
    };
    // Only what was left out changed: nothing to commit.
    if staged.exit_code == 0 {
        return AutoCommitResult { outcome: Nothing, left_out, why: None };
    }
    // An app commit never waits on a signing prompt or a hook.
    let text = message_for(child_id, revision);
    let arguments = ["-c", "commit.gpgsign=false", "commit", "-q", "--no-verify", "-m", &text];
    if let Some(failed) = write_step(git, worktree_path, &lock_file, &arguments, None, TIMEOUT).await {
        return AutoCommitResult::failing(failed, left_out);
    }
    AutoCommitResult { outcome: Committed, left_out, why: None }
}

/// `ChildAutoCommit.WriteStep`: one git write of the commit, None when it went through, otherwise why not. Only a git the app itself
/// killed on its deadline has its `index.lock` removed, since that lock is its own; a lock from any other failure (another git working
/// there, one that never started, one that stopped reading its input) is left as it is.
pub async fn write_step(
    git: &Git,
    worktree_path: &str,
    lock_file: &str,
    arguments: &[&str],
    input: Option<&str>,
    timeout: f64,
) -> Option<String> {
    let run = git.run_detailed(arguments, Path::new(worktree_path), timeout, input).await;
    match run.failure {
        GitFailure::None => run.result.filter(|result| result.exit_code != 0).map(|result| message(&result)),
        GitFailure::TimedOut => {
            let _ = std::fs::remove_file(lock_file);
            Some("Git did not finish in time.".into())
        }
        GitFailure::NotStarted => Some("Git could not be started.".into()),
        GitFailure::BrokenPipe => Some(match run.result.filter(|result| result.exit_code != 0) {
            Some(stopped) => message(&stopped),
            None => "Git stopped reading the list of new files.".into(),
        }),
        GitFailure::TooMuchOutput | GitFailure::Unreadable => Some("Git did not finish.".into()),
    }
}

/// `Size`: the size of the file at `path`, not following a link; 0 when unknown.
fn size(path: &str) -> u64 {
    std::fs::symlink_metadata(path).ok().filter(|meta| meta.is_file()).map_or(0, |meta| meta.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secret_looking_names_and_the_commit_message() {
        // C# DelegationHumanChildVerification.cs:103-105, then a few more.
        for name in [
            ".env",
            ".ENV.production",
            "a/b/cert.pem",
            "x.key",
            "id_rsa.pub",
            "credentials",
            r"keys\deploy.key",
            "cert.PFX",
            "store.p12",
            "putty.ppk",
            "id_ed25519",
            "id_ed25519.pub",
            "id_ecdsa",
            "id_dsa.pub",
            ".npmrc",
            "home/.pypirc",
            "secrets.json",
            "Secrets.yaml",
            ".env.local",
            "keys/server.PEM",
            "id_rsa",
            "Credentials.json",
            // ToLowerInvariant reads U+0130 as `i`.
            "\u{130}D_RSA",
        ] {
            assert!(looks_secret(name), "{name} looks secret");
        }
        for name in [
            "environment.txt",
            "keyboard.swift",
            "a/.envoy/config",
            "my-credentials.txt",
            "npmrc",
            ".npmrc.example",
            "my-secrets.json",
            "secrets",
            "pfx.txt",
            "config.json",
            "my.env",
            "keys/",
            "secret.txt",
        ] {
            assert!(!looks_secret(name), "{name} does not");
        }
        assert_eq!(to_lower_invariant("\u{130}A\u{3A3}"), "ia\u{3C3}");
        assert_eq!(message_for("c1", 3), "mighty: c1 report r3");
        assert!(should_commit("codex", true, true, false) && !should_commit("claude", true, true, false));
        assert!(
            !should_commit("gemini", false, true, false)
                && !should_commit("gemini", true, false, false)
                && !should_commit("gemini", true, true, true)
        );
    }
}
