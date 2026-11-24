//! 바뀐 부분별 받기·버리기 in a worktree's changes sheet (MC-30): the worktree pane's 변경 보기, a delegated child's from the review
//! queue and a compare run's diff (all `paneWorktreeChanges`). The base of a file is its copy in the commit the worktree shares with
//! its base branch (as the sheet's diff reads it), the file now is the worktree's; the rules are mighty-files' (`hunks`, `revert`):
//! a hunk put back writes only its lines (refused as `conflict` when they moved off or changed), the last one put back trashes a file
//! the base did not have, and a hunk taken is only recorded, in the worktree's book under the app's data (files.rs `worktree_book`).
//!
//! - `paneWorktreeHunks{id, path}`: the file's diff and each hunk's fingerprint and decision; null when git cannot read the base.
//! - `paneWorktreeHunk{id, path?, action: discard|accept|acceptAll, fingerprints?, hunk?, paths?}`: one hunk put back, hunks taken,
//!   or every hunk left in the files taken; answers `{outcome?, review?}`.
//! - `paneWorktreeReview{id, paths}`: each file's progress (`{path: {handled, total, reviewed}}`), at most 64 files.
//!
//! MC-47: an SSH pane's worktree is on its machine: the shared commit is read there, a file's base is `git cat-file blob` there, the
//! file now is read there, and a hunk put back is the MC-40 review's atomic write over ssh (`conflict` when the file changed since it
//! was read), a new file's last hunk its move to the store's trash over there (mighty-files `remote_review::RemoteFolder`, the same
//! `revert::discard_on` rules). The decisions are kept here, as a local worktree's.

use std::path::Path;
use std::sync::Arc;

use mighty_core::workspace_files;
use mighty_delegation::pane_worktree as core;
use mighty_files::changes;
use mighty_files::classify::decode_text;
use mighty_files::diff;
use mighty_files::hunks::{self, Decision, ReviewBook};
use mighty_files::revert::{self, Base, Later, Refusal};
use serde_json::{Value, json};

use crate::files::string_list;
use crate::host::{Host, Registry};
use crate::methods::text as arg;

/// The most files one progress reading looks at.
const MAXIMUM_FILES: usize = 64;

/// One worktree as its hunks are read: its folder (real), the commit it shares with its base, and (MC-47) its machine's files when it
/// is an SSH pane's.
struct Worktree {
    root: String,
    since: String,
    remote: Option<Remote>,
}

/// An SSH pane's worktree on its machine.
#[derive(Clone)]
struct Remote {
    folder: mighty_run::ssh_git::SshFolder,
    files: Arc<mighty_files::remote_review::RemoteFolder>,
}

/// One file of it: its path as the disk spells it, its base (None when git's copy is no text) and the shared commit's short name.
struct WorktreeFile {
    path: String,
    base: Option<Base>,
    short: String,
}

async fn worktree(host: &Arc<Host>, id: &str) -> Option<Worktree> {
    let place = crate::pane_worktree::place_of(host, id).await?;
    let pane = crate::pane_worktree::review_record(host, id)?;
    let since = core::merge_base(place.machine(), &pane).await?;
    if let Some(ssh) = &place.ssh {
        let root = place.machine().standardized(pane.worktree_path());
        let exec = Arc::new(mighty_files::remote_review::SshExec {
            processes: host.processes.clone(),
            launch: ssh.launch.clone(),
            handle: tokio::runtime::Handle::current(),
        });
        let files = Arc::new(mighty_files::remote_review::RemoteFolder::new(exec, &root));
        let folder = mighty_run::ssh_git::SshFolder::new(host.processes.clone(), ssh.launch.clone());
        return Some(Worktree { root, since, remote: Some(Remote { folder, files }) });
    }
    let root = workspace_files::real_path(Path::new(pane.worktree_path()))?.to_string_lossy().into_owned();
    Some(Worktree { root, since, remote: None })
}

impl Worktree {
    /// The file `path` names inside the worktree, with its base read from git; None when it leaves the worktree or git cannot say.
    async fn file(&self, host: &Host, path: &str) -> Option<WorktreeFile> {
        if path.is_empty() || path.contains('\0') || path.len() > 4096 {
            return None;
        }
        if let Some(remote) = &self.remote {
            return remote_file(remote, &self.root, &self.since, path).await;
        }
        let root = self.root.clone();
        let wanted = path.to_string();
        let relative = tauri::async_runtime::spawn_blocking(move || {
            let real_root = Path::new(&root);
            // A deleted file no longer resolves: its path is kept as git wrote it.
            match workspace_files::resolve(&wanted, real_root) {
                Some(real) => workspace_files::relative(&real, real_root).map(|r| changes::on_disk_case(real_root, &r.replace('\\', "/"))),
                None => (!wanted.split(['/', '\\']).any(|p| matches!(p, "" | "." | ".."))).then(|| wanted.replace('\\', "/")),
            }
        })
        .await
        .ok()
        .flatten()?;
        let blob = changes::git_blob(host.processes.as_ref(), &self.root, &self.since, &relative, diff::MAXIMUM_SIDE_BYTES).await?;
        let base = if blob.untracked {
            Some(Base { text: String::new(), created: true })
        } else {
            decode_text(&blob.data, false).map(|d| Base { text: d.text, created: false })
        };
        Some(WorktreeFile { path: relative, base, short: self.since[..7].to_string() })
    }
}

/// MC-47: a file of an SSH pane's worktree with its base: `git cat-file blob <since>:./<path>` over there (a path the commit does
/// not have is a new file, as `changes::git_blob` reads it).
async fn remote_file(remote: &Remote, root: &str, since: &str, path: &str) -> Option<WorktreeFile> {
    use mighty_core::ssh::worktree::{GitKind, git_script};
    let relative = path.replace('\\', "/");
    if !mighty_core::ssh::files::valid_relative(&relative) || relative.is_empty() || !(40..=64).contains(&since.len()) {
        return None;
    }
    let spec = format!("{since}:./{relative}");
    let script = git_script(root, GitKind::Delegation, &["cat-file", "blob", &spec], 10)?;
    let ended = remote.folder.exec(&script, None, std::time::Duration::from_secs(40), diff::MAXIMUM_SIDE_BYTES).await.ok()?;
    let base = match ended.code {
        0 => decode_text(&ended.stdout, false).map(|d| Base { text: d.text, created: false }),
        _ => {
            let error = String::from_utf8_lossy(&ended.stderr);
            if !(error.contains("does not exist in '") || error.contains("exists on disk, but not in '")) {
                return None;
            }
            Some(Base { text: String::new(), created: true })
        }
    };
    Some(WorktreeFile { path: relative, base, short: since[..7].to_string() })
}

/// The file now: through the files pane's open (None when it is not text the diff reads); an SSH worktree's read over there.
fn current(host: &Host, tree: &Worktree, path: &str) -> Option<String> {
    if let Some(remote) = &tree.remote {
        return revert::ReviewFiles::current(remote.files.as_ref(), path);
    }
    let files = crate::files::state(host).ok()?;
    changes::current(path, Path::new(&tree.root), files.ports.handles.as_ref())
}

/// Whether the file is a regular file of the worktree (no link).
fn regular(tree: &Worktree, path: &str) -> bool {
    match &tree.remote {
        Some(remote) => revert::ReviewFiles::regular(remote.files.as_ref(), path),
        None => revert::regular_file(path, &tree.root).is_some(),
    }
}

/// The file's review for the page (`revert::file_hunks_json`, the base named as `head` with the shared commit's short name).
fn review(host: &Host, tree: &Worktree, file: &WorktreeFile, book: &ReviewBook) -> Value {
    let reading = (|| {
        regular(tree, &file.path).then_some(()).ok_or(Refusal::Missing)?;
        let base = file.base.as_ref().ok_or(Refusal::Unreadable)?;
        let now = current(host, tree, &file.path).ok_or(Refusal::Unreadable)?;
        revert::file_hunks(&base.text, &now, &book.decisions(&file.path), &Later::Nothing).ok_or(Refusal::Unreadable)
    })();
    let created = file.base.as_ref().is_some_and(|b| b.created);
    let mut answer = revert::file_hunks_json(reading, "head", None, created);
    if let Some(changes) = answer.get_mut("changes") {
        changes["headSha"] = json!(file.short);
    }
    answer
}

/// The fingerprints of the file's hunks now.
fn prints(host: &Host, tree: &Worktree, file: &WorktreeFile) -> Vec<String> {
    let Some(base) = &file.base else { return vec![] };
    current(host, tree, &file.path).and_then(|now| hunks::hunks(&base.text, &now)).map(|(_, prints)| prints).unwrap_or_default()
}

pub fn register(methods: &mut Registry) {
    methods.add("paneWorktreeHunks", |host: Arc<Host>, args| async move {
        let (Some(id), Some(path)) = (arg(&args, "id"), arg(&args, "path")) else { return Ok(Value::Null) };
        let Some(tree) = worktree(&host, id).await else { return Ok(Value::Null) };
        let Some(file) = tree.file(&host, path).await else { return Ok(Value::Null) };
        let book = crate::files::state(&host)?.worktree_book(id);
        let reader = host.clone();
        tauri::async_runtime::spawn_blocking(move || review(&reader, &tree, &file, &book)).await.map_err(|e| e.to_string())
    });
    methods.add("paneWorktreeHunk", |host: Arc<Host>, args| async move {
        let Some(id) = arg(&args, "id") else { return Ok(Value::Null) };
        let Some(action) = arg(&args, "action").filter(|a| matches!(*a, "discard" | "accept" | "acceptAll")).map(str::to_string) else {
            return Ok(Value::Null);
        };
        let Some(tree) = worktree(&host, id).await else { return Ok(Value::Null) };
        let files = crate::files::state(&host)?;
        let book = files.worktree_book(id);
        let fingerprints = string_list(&args, "fingerprints", 1_000);
        let shown = args.get("hunk").and_then(diff::Hunk::from_json);
        let mut answer = json!({});
        if action == "acceptAll" {
            let mut paths = string_list(&args, "paths", MAXIMUM_FILES);
            paths.extend(arg(&args, "path").map(str::to_string).filter(|_| paths.is_empty()));
            for path in paths {
                let Some(file) = tree.file(&host, &path).await else { continue };
                let reader = host.clone();
                let tree = Worktree { root: tree.root.clone(), since: tree.since.clone(), remote: tree.remote.clone() };
                let book = book.clone();
                let _ = tauri::async_runtime::spawn_blocking(move || {
                    let now = prints(&reader, &tree, &file);
                    if !now.is_empty() {
                        let _ = book.record(&file.path, &now, Decision::Accepted);
                    }
                })
                .await;
            }
        }
        let Some(path) = arg(&args, "path") else { return Ok(answer) };
        let Some(file) = tree.file(&host, path).await else { return Ok(Value::Null) };
        let reader = host.clone();
        let desktop = files.ports.desktop.clone();
        let answer = tauri::async_runtime::spawn_blocking(move || {
            match action.as_str() {
                "discard" => {
                    let outcome = match (&file.base, fingerprints.first()) {
                        // MC-47: an SSH worktree's hunk goes back over there (the same rules, the MC-40 atomic write and trash).
                        (Some(base), Some(fingerprint)) if tree.remote.is_some() => {
                            let files = tree.remote.as_ref().map(|r| r.files.clone()).expect("remote");
                            files.forget();
                            let now = current(&reader, &tree, &file.path);
                            revert::discard_on(
                                files.as_ref(),
                                &file.path,
                                base,
                                now,
                                |_| Later::Nothing,
                                fingerprint,
                                shown.as_ref(),
                                Some(&book),
                            )
                        }
                        (Some(base), Some(fingerprint)) => {
                            let now = current(&reader, &tree, &file.path);
                            let trash = move |path: &Path| desktop.trash(path).map_err(|e| e.to_string());
                            revert::discard_in(
                                &tree.root,
                                &file.path,
                                base,
                                now,
                                |_| Later::Nothing,
                                fingerprint,
                                shown.as_ref(),
                                Some(&book),
                                &trash,
                            )
                        }
                        (None, _) => revert::Outcome::Refused(Refusal::Unreadable),
                        (_, None) => revert::Outcome::Refused(Refusal::Conflict),
                    };
                    answer["outcome"] = outcome.to_json();
                }
                "accept" if !fingerprints.is_empty() => {
                    answer["saved"] = json!(book.record(&file.path, &fingerprints, Decision::Accepted).is_ok());
                }
                _ => {}
            }
            answer["review"] = review(&reader, &tree, &file, &book);
            answer
        })
        .await
        .map_err(|e| e.to_string())?;
        Ok(answer)
    });
    methods.add("paneWorktreeReview", |host: Arc<Host>, args| async move {
        let Some(id) = arg(&args, "id") else { return Ok(Value::Null) };
        let Some(tree) = worktree(&host, id).await else { return Ok(Value::Null) };
        let book = crate::files::state(&host)?.worktree_book(id);
        let entries = book.read();
        let mut out = serde_json::Map::new();
        for path in string_list(&args, "paths", MAXIMUM_FILES) {
            let Some(file) = tree.file(&host, &path).await else { continue };
            let reader = host.clone();
            let tree = Worktree { root: tree.root.clone(), since: tree.since.clone(), remote: tree.remote.clone() };
            let decisions = hunks::decisions_of(&entries, &file.path);
            let progress = tauri::async_runtime::spawn_blocking(move || {
                file.base.as_ref()?;
                regular(&tree, &file.path).then_some(())?;
                Some(hunks::progress(&prints(&reader, &tree, &file), &decisions))
            })
            .await
            .ok()
            .flatten();
            if let Some(progress) = progress {
                out.insert(path, json!({"handled": progress.handled, "total": progress.total, "reviewed": progress.reviewed()}));
            }
        }
        Ok(Value::Object(out))
    });
}
