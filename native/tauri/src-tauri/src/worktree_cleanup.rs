//! MC-80: worktrees left behind. Right after a merge the app cleans the worktree up itself (delegation.rs, pane_worktree.rs, with
//! [`leftover_message`] when files would go with it). What earlier versions, a crash or a hand-made change left in the worktree root
//! (`~/.mightycode/worktrees`) or as `mighty/<id>` branches in the workspaces' repositories, with no pane, kept worktree or delegated
//! child naming it, is found a little after the app starts and offered for removal once, each one's unmerged commits or uncommitted
//! files named.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use mighty_core::locale::Locale;
use mighty_delegation::git::{Git, run};
use serde_json::json;

use crate::host::Host;

const GIT_TIMEOUT: f64 = 30.0;
/// The files a cleanup question names before it says how many more there are.
const NAMED_FILES: usize = 10;

/// The question's words for files left uncommitted in a merged worktree: up to ten of them, then how many more.
pub fn leftover_message(locale: &Locale, files: &[String]) -> String {
    let mut lines: Vec<String> = files.iter().take(NAMED_FILES).map(|f| format!("• {f}")).collect();
    if files.len() > NAMED_FILES {
        lines.push(locale.get_with("worktree.mergedCleanup.more", &[("count", &(files.len() - NAMED_FILES).to_string())]));
    }
    format!(
        "{}\n\n{}\n\n{}",
        locale.get_with("worktree.mergedCleanup.message", &[("count", &files.len().to_string())]),
        lines.join("\n"),
        locale.get("worktree.mergedCleanup.question")
    )
}

/// One leftover: a worktree some repository still lists, a folder at the root no repository does, or a branch alone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Leftover {
    pub id: String,
    /// The repository that lists it (None for a folder no repository does).
    pub repo: Option<String>,
    /// Its folder under the root, when there is one.
    pub folder: Option<String>,
    /// Its branch `mighty/<id>` in `repo`, when there is one.
    pub branch: Option<String>,
    /// What would be lost: commits on the branch its repository's HEAD does not hold, files not committed in the folder.
    pub unmerged: bool,
    pub uncommitted: bool,
}

/// Which of the worktrees and branches found are left over: an id no pane, kept worktree or live child names, a folder directly under
/// the root, the app's own `mighty/<id>` names only.
pub fn select(
    root: &Path,
    known: &HashSet<String>,
    listed: &[(String, String, Option<String>)],
    folders: &[String],
    branches: &[(String, String)],
) -> Vec<Leftover> {
    let unknown = |id: &str| safe_id(id) && !known.contains(&id.to_lowercase());
    let mut found: Vec<Leftover> = Vec::new();
    for (repo, path, branch) in listed {
        let path_buf = PathBuf::from(path);
        if path_buf.parent() != Some(root) {
            continue;
        }
        let Some(id) = path_buf.file_name().and_then(|n| n.to_str()) else { continue };
        if !unknown(id) || found.iter().any(|f| f.id.eq_ignore_ascii_case(id)) {
            continue;
        }
        let branch = branch.clone().filter(|b| *b == format!("mighty/{id}"));
        found.push(Leftover {
            id: id.to_string(),
            repo: Some(repo.clone()),
            folder: Some(path.clone()),
            branch,
            unmerged: false,
            uncommitted: false,
        });
    }
    for folder in folders {
        let path_buf = PathBuf::from(folder);
        let Some(id) = path_buf.file_name().and_then(|n| n.to_str()) else { continue };
        if path_buf.parent() != Some(root) || !unknown(id) || found.iter().any(|f| f.id.eq_ignore_ascii_case(id)) {
            continue;
        }
        found.push(Leftover {
            id: id.to_string(),
            repo: None,
            folder: Some(folder.clone()),
            branch: None,
            unmerged: false,
            uncommitted: false,
        });
    }
    for (repo, branch) in branches {
        let Some(id) = branch.strip_prefix("mighty/") else { continue };
        if !unknown(id) {
            continue;
        }
        match found.iter_mut().find(|f| f.id.eq_ignore_ascii_case(id) && f.repo.as_deref().is_none_or(|r| r == repo)) {
            Some(existing) => {
                existing.repo = Some(repo.clone());
                existing.branch = Some(branch.clone());
            }
            None => found.push(Leftover {
                id: id.to_string(),
                repo: Some(repo.clone()),
                folder: None,
                branch: Some(branch.clone()),
                unmerged: false,
                uncommitted: false,
            }),
        }
    }
    found
}

/// An id the app makes for a worktree: letters, digits and `-` only, as a session id is.
fn safe_id(id: &str) -> bool {
    (8..=80).contains(&id.len()) && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
}

/// A little after the app starts (when it may write): the leftovers, offered for removal once.
pub fn start(host: &Arc<Host>) {
    if host.smoke.is_some() || !host.delegation_live() {
        return;
    }
    let host = host.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(Duration::from_secs(20)).await;
        let Some((git, root)) = crate::pane_worktree::places().map(|p| (p.git.clone(), p.root.clone())) else { return };
        let found = find(&host, &git, &root).await;
        if found.is_empty() || host.closing() {
            return;
        }
        let locale = host.locale;
        let dialog = json!({
            "type": "confirm", "title": locale.get("worktree.leftovers.title"),
            "message": message(&locale, &found),
            "primary": locale.get("worktree.leftovers.confirm"), "close": locale.get("worktree.leftovers.later"),
            "defaultPrimary": false, "destructive": true,
        });
        if !crate::delegation::ask_confirm(&host, dialog).await || host.closing() {
            return;
        }
        let failed = remove(&git, &root, &found).await;
        let words = if failed.is_empty() {
            locale.get_with("worktree.leftovers.done", &[("count", &found.len().to_string())])
        } else {
            locale.get_with("worktree.leftovers.failed", &[("reason", &failed.join("; "))])
        };
        crate::pane_worktree::banner(&host, words);
    });
}

/// The leftovers now: the ids that are known (every pane, every kept worktree, every child not discarded), the worktrees and
/// `mighty/*` branches of the workspaces' repositories on this computer, and the folders under the root.
async fn find(host: &Arc<Host>, git: &Git, root: &str) -> Vec<Leftover> {
    let (mut known, repos) = {
        let view = host.view();
        let mut known: HashSet<String> = view.snapshot.sessions().iter().map(|s| s.id().to_lowercase()).collect();
        let mut repos: Vec<String> = Vec::new();
        for w in view.snapshot.workspaces() {
            known.extend(w.kept_pane_worktrees.iter().flatten().map(|k| k.session_id().to_lowercase()));
            if w.remote.is_none() && w.ssh.is_none() && !w.is_bot_home() && Path::new(w.path()).is_dir() {
                repos.push(w.path().to_string());
            }
        }
        (known, repos)
    };
    if let Some(app) = crate::delegation::app() {
        known.extend(app.live_child_ids().into_iter().map(|id| id.to_lowercase()));
    }
    let root_path = std::fs::canonicalize(root).unwrap_or_else(|_| PathBuf::from(root));
    let mut listed = Vec::new();
    let mut branches = Vec::new();
    for repo in &repos {
        if let Some(out) = run(git, &["worktree", "list", "--porcelain"], repo, GIT_TIMEOUT).await.filter(|r| r.exit_code == 0) {
            let (mut path, mut branch): (Option<String>, Option<String>) = (None, None);
            for line in out.stdout.lines().chain(std::iter::once("")) {
                if let Some(p) = line.strip_prefix("worktree ") {
                    path = Some(std::fs::canonicalize(p).map(|c| c.to_string_lossy().into_owned()).unwrap_or_else(|_| p.to_string()));
                } else if let Some(b) = line.strip_prefix("branch refs/heads/") {
                    branch = Some(b.to_string());
                } else if line.is_empty() {
                    if let Some(p) = path.take() {
                        listed.push((repo.clone(), p, branch.take()));
                    }
                    branch = None;
                }
            }
        }
        if let Some(out) = run(git, &["for-each-ref", "--format=%(refname:short)", "refs/heads/mighty/"], repo, GIT_TIMEOUT)
            .await
            .filter(|r| r.exit_code == 0)
        {
            branches.extend(out.stdout.lines().filter(|l| !l.trim().is_empty()).map(|l| (repo.clone(), l.trim().to_string())));
        }
    }
    let folders: Vec<String> = std::fs::read_dir(&root_path)
        .map(|entries| {
            entries.flatten().filter(|e| e.file_type().is_ok_and(|t| t.is_dir())).map(|e| e.path().to_string_lossy().into_owned()).collect()
        })
        .unwrap_or_default();
    let mut found = select(&root_path, &known, &listed, &folders, &branches);
    for leftover in &mut found {
        if let (Some(repo), Some(branch)) = (&leftover.repo, &leftover.branch) {
            leftover.unmerged =
                run(git, &["merge-base", "--is-ancestor", branch, "HEAD"], repo, GIT_TIMEOUT).await.is_some_and(|r| r.exit_code == 1);
        }
        if let Some(folder) = leftover.folder.as_deref().filter(|f| Path::new(f).join(".git").exists()) {
            leftover.uncommitted =
                run(git, &["status", "--porcelain"], folder, GIT_TIMEOUT).await.is_some_and(|r| !r.stdout.trim().is_empty());
        }
    }
    found
}

fn message(locale: &Locale, found: &[Leftover]) -> String {
    let lines: Vec<String> = found
        .iter()
        .take(NAMED_FILES)
        .map(|f| {
            let short: String = f.id.chars().take(8).collect();
            let place =
                f.repo.as_deref().and_then(|r| Path::new(r).file_name()).map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            let mut marks = Vec::new();
            if f.unmerged {
                marks.push(locale.get("worktree.leftovers.unmerged"));
            }
            if f.uncommitted {
                marks.push(locale.get("worktree.leftovers.uncommitted"));
            }
            let what = match (&f.folder, &f.branch) {
                (Some(_), Some(_)) => locale.get("worktree.leftovers.both"),
                (Some(_), None) => locale.get("worktree.leftovers.folder"),
                _ => locale.get("worktree.leftovers.branch"),
            };
            let place = if place.is_empty() { String::new() } else { format!(" ({place})") };
            let marks = if marks.is_empty() { String::new() } else { format!(" · {}", marks.join(", ")) };
            format!("• {short}{place} · {what}{marks}")
        })
        .collect();
    let more = if found.len() > NAMED_FILES {
        format!("\n{}", locale.get_with("worktree.mergedCleanup.more", &[("count", &(found.len() - NAMED_FILES).to_string())]))
    } else {
        String::new()
    };
    format!(
        "{}\n\n{}{more}\n\n{}",
        locale.get_with("worktree.leftovers.message", &[("count", &found.len().to_string())]),
        lines.join("\n"),
        locale.get("worktree.leftovers.question")
    )
}

/// Removes the leftovers: a listed worktree by `git worktree remove --force --force` from its repository, a folder no repository lists
/// (directly under the root, not a link) with what is in it, a branch by `git branch -D`; then each repository's worktree list pruned.
/// What could not be removed, in words.
async fn remove(git: &Git, root: &str, found: &[Leftover]) -> Vec<String> {
    let root_path = std::fs::canonicalize(root).unwrap_or_else(|_| PathBuf::from(root));
    let mut failed = Vec::new();
    let mut repos: HashSet<String> = HashSet::new();
    for leftover in found {
        if let Some(repo) = &leftover.repo {
            repos.insert(repo.clone());
        }
        if let Some(folder) = &leftover.folder {
            let path = Path::new(folder);
            let listed = leftover.repo.is_some();
            if listed {
                let repo = leftover.repo.as_deref().unwrap_or_default();
                let removed = run(git, &["worktree", "remove", "--force", "--force", folder], repo, GIT_TIMEOUT).await;
                if removed.as_ref().is_none_or(|r| r.exit_code != 0) && path.exists() {
                    failed.push(match &removed {
                        Some(r) => format!("{} ({})", leftover.id, mighty_delegation::git::message(r)),
                        None => leftover.id.clone(),
                    });
                }
            }
            let is_link = std::fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink());
            if path.exists() && !is_link && path.parent() == Some(root_path.as_path()) && std::fs::remove_dir_all(path).is_err() {
                failed.push(leftover.id.clone());
            }
        }
    }
    for repo in &repos {
        let _ = run(git, &["worktree", "prune"], repo, GIT_TIMEOUT).await;
    }
    for leftover in found {
        if let (Some(repo), Some(branch)) = (&leftover.repo, &leftover.branch)
            && run(git, &["branch", "-D", branch], repo, GIT_TIMEOUT).await.is_none_or(|r| r.exit_code != 0)
        {
            failed.push(branch.clone());
        }
    }
    failed
}

#[cfg(test)]
mod tests {
    use super::*;

    /// MC-80: only what nothing names is left over: a listed worktree, a folder no repository lists, a lone `mighty/<id>` branch;
    /// a known id, a place outside the root and another branch name never are.
    #[test]
    fn only_what_nothing_names_is_left_over() {
        let root = Path::new("/home/u/.mightycode/worktrees");
        let known: HashSet<String> = ["aaaaaaaa-1111".to_string()].into_iter().collect();
        let listed = vec![
            ("/repo".to_string(), "/repo".to_string(), Some("main".to_string())),
            ("/repo".to_string(), "/home/u/.mightycode/worktrees/AAAAAAAA-1111".to_string(), Some("mighty/AAAAAAAA-1111".to_string())),
            ("/repo".to_string(), "/home/u/.mightycode/worktrees/bbbbbbbb-2222".to_string(), Some("mighty/bbbbbbbb-2222".to_string())),
            ("/repo".to_string(), "/elsewhere/cccccccc-3333".to_string(), Some("mighty/cccccccc-3333".to_string())),
        ];
        let folders = vec![
            "/home/u/.mightycode/worktrees/bbbbbbbb-2222".to_string(),
            "/home/u/.mightycode/worktrees/dddddddd-4444".to_string(),
            "/home/u/.mightycode/worktrees/.DS_Store".to_string(),
        ];
        let branches = vec![
            ("/repo".to_string(), "mighty/bbbbbbbb-2222".to_string()),
            ("/repo".to_string(), "mighty/eeeeeeee-5555".to_string()),
            ("/repo".to_string(), "mighty/aaaaaaaa-1111".to_string()),
            ("/repo".to_string(), "feature/x".to_string()),
        ];
        let found = select(root, &known, &listed, &folders, &branches);
        let ids: Vec<&str> = found.iter().map(|f| f.id.as_str()).collect();
        assert_eq!(ids, ["bbbbbbbb-2222", "dddddddd-4444", "eeeeeeee-5555"]);
        assert_eq!(found[0].branch.as_deref(), Some("mighty/bbbbbbbb-2222"), "its branch goes with it");
        assert!(found[1].repo.is_none() && found[1].branch.is_none(), "a folder no repository lists");
        assert!(found[2].folder.is_none() && found[2].repo.as_deref() == Some("/repo"), "a branch alone");
    }

    #[test]
    fn a_merged_worktrees_files_are_named_up_to_ten() {
        let locale = Locale::new("en");
        let files: Vec<String> = (0..12).map(|i| format!("f{i}.txt")).collect();
        let text = leftover_message(&locale, &files);
        assert!(text.contains("• f9.txt") && !text.contains("• f10.txt"));
        assert!(!text.contains("worktree.mergedCleanup"), "its words are in the locale: {text}");
    }
}
