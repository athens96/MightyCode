//! 요청 되돌리기 (M/MightyCore/RunRevert.swift): puts the files one request changed back the way they were before it, from the originals
//! [`FileOriginalStore`] kept for that run. What a revert writes is exactly what the 변경사항 view shows as the run's base, so the review
//! (요청 변경 검토) shows what will go before the person confirms.
//!
//! A path is refused, never half-written, when:
//! - a later run of the same pane changed it too ([`Refusal::LaterRun`]): undoing the earlier run would also undo that one's work, so the
//!   newer run goes first;
//! - the file is gone or no longer a regular file inside the folder ([`Refusal::Missing`]);
//! - the file is no longer text the diff can read ([`Refusal::Unreadable`]): over 1 MiB, binary or undecodable;
//! - it already reads as the original ([`Refusal::Unchanged`]).
//!
//! One hunk at a time (바뀐 부분별 받기·버리기, [`hunk_review`], [`discard_hunk`], [`accept_hunks`]; the rules in [`crate::hunks`]): a
//! hunk is refused when its lines changed after the review read them ([`Refusal::Conflict`]) or when a later run of the pane changed
//! lines it touches ([`Refusal::LaterRun`], narrowed from the file to those lines; the whole file when that run kept no original of
//! it). Putting back the last hunk does what reverting the file does: the original written back, or a file the run made trashed.
//!
//! A file the run created is moved to the Trash (the Recycle Bin) through the caller's `trash`, never deleted. A restored file keeps the
//! encoding it is in now (its byte order mark too: the store keeps originals without one) and its permissions; its line endings are the
//! original's, which the store kept as they were.

use std::collections::HashMap;
use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};

use mighty_composer::text::{TextEncodingKind, byte_order_mark, decode_text};
use mighty_core::workspace_files;
use serde_json::{Map, Value, json};

use crate::changes::{FileOriginal, FileOriginalStore, same_path, source_run};
use crate::diff::{self, DiffResult, Hunk};
use crate::hunks::{self, Decision, HunkState, Progress, ReviewBook};

/// Why a path cannot be reverted now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    LaterRun,
    Missing,
    Unreadable,
    Unchanged,
    /// 바뀐 부분별 버리기: the hunk's lines are no longer in the file as the review read them.
    Conflict,
}

impl Refusal {
    pub fn word(self) -> &'static str {
        match self {
            Refusal::LaterRun => "laterRun",
            Refusal::Missing => "missing",
            Refusal::Unreadable => "unreadable",
            Refusal::Unchanged => "unchanged",
            Refusal::Conflict => "conflict",
        }
    }
}

/// What reverting a path does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Write the original text back.
    Restore,
    /// The run made the file: move it to the Trash.
    Trash,
}

/// One file the run changed, as reverting it would go.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    pub path: String,
    pub action: Action,
    /// Lines the revert puts back (the run removed them) and takes out (the run added them).
    pub restored: usize,
    pub removed: usize,
    pub refusal: Option<Refusal>,
    /// 바뀐 부분별 받기·버리기: of the file's hunks (now and put back), how many were taken or put back.
    pub progress: Progress,
}

impl Item {
    pub fn revertible(&self) -> bool {
        self.refusal.is_none()
    }

    /// 검토됨: every hunk was taken or put back, or nothing is left to review (the file reads as its original, or is gone).
    pub fn reviewed(&self) -> bool {
        match self.refusal {
            Some(Refusal::Unchanged | Refusal::Missing) => true,
            Some(Refusal::Unreadable) => false,
            _ => self.progress.reviewed(),
        }
    }

    /// `{path, action, restored, removed, hunks, handled, reviewed, refusal?}` for the page.
    pub fn to_json(&self) -> Value {
        let mut out = Map::new();
        out.insert("path".into(), json!(self.path));
        out.insert("action".into(), json!(if self.action == Action::Trash { "trash" } else { "restore" }));
        out.insert("restored".into(), json!(self.restored));
        out.insert("removed".into(), json!(self.removed));
        out.insert("hunks".into(), json!(self.progress.total));
        out.insert("handled".into(), json!(self.progress.handled));
        out.insert("reviewed".into(), json!(self.reviewed()));
        if let Some(refusal) = self.refusal {
            out.insert("refusal".into(), json!(refusal.word()));
        }
        Value::Object(out)
    }
}

/// How one path's revert went.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Reverted,
    Refused(Refusal),
    Failed(String),
}

impl Outcome {
    /// `{kind: 'reverted'|'refused'|'failed', refusal?, error?}` for the page.
    pub fn to_json(&self) -> Value {
        match self {
            Outcome::Reverted => json!({"kind": "reverted"}),
            Outcome::Refused(refusal) => json!({"kind": "refused", "refusal": refusal.word()}),
            Outcome::Failed(error) => json!({"kind": "failed", "error": error}),
        }
    }
}

/// What reverting `run_id` (a notice run: the run it came from) of the pane `session_id` would do to each file it changed under `root`,
/// in the order the run changed them.
pub fn plan(store: &FileOriginalStore, session_id: &str, run_id: &str, root: &str) -> Vec<Item> {
    let run_id = source_run(run_id);
    let later = later_paths(store, session_id, run_id, root);
    let book = store.review_book(session_id, run_id, root).map(|b| b.read()).unwrap_or_default();
    store
        .files(session_id, run_id, root)
        .into_iter()
        .filter_map(|path| {
            let original = store.find(session_id, Some(run_id), &path, Some(root))?;
            let decisions = hunks::decisions_of(&book, &path);
            Some(item(store, &path, &original, root, later.iter().any(|p| same_path(p, &path)), &decisions))
        })
        .collect()
}

/// Reverts the given paths of `run_id` (all it changed when None). Each path is checked again just before it is written; one refusal
/// or failure never stops the others. `trash` moves a file the run created to the OS's Trash.
pub fn apply(
    store: &FileOriginalStore,
    session_id: &str,
    run_id: &str,
    root: &str,
    paths: Option<&[String]>,
    trash: &dyn Fn(&Path) -> Result<(), String>,
) -> Vec<(String, Outcome)> {
    let run_id = source_run(run_id);
    let later = later_paths(store, session_id, run_id, root);
    let mut outcomes = Vec::new();
    for path in store.files(session_id, run_id, root) {
        if paths.is_some_and(|wanted| !wanted.iter().any(|w| same_path(w, &path))) {
            continue;
        }
        let Some(original) = store.find(session_id, Some(run_id), &path, Some(root)) else { continue };
        let planned = item(store, &path, &original, root, later.iter().any(|p| same_path(p, &path)), &HashMap::new());
        if let Some(refusal) = planned.refusal {
            outcomes.push((path, Outcome::Refused(refusal)));
            continue;
        }
        let Some(file) = regular_file(&path, root) else {
            outcomes.push((path, Outcome::Refused(Refusal::Missing)));
            continue;
        };
        let done = match planned.action {
            Action::Trash => trash(&file),
            Action::Restore => write_if(&original.text, &file, None).map(|_| ()).map_err(|e| e.to_string()),
        };
        outcomes.push((path, done.map_or_else(Outcome::Failed, |()| Outcome::Reverted)));
    }
    outcomes
}

// ---- 바뀐 부분별 받기·버리기 ----------------------------------------------------------------------------------------------------

/// A file's base for a hunk review: its text before (a run's original, a worktree's merge base) and whether the file is new.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Base {
    pub text: String,
    pub created: bool,
}

/// What later changes of the file a hunk must keep clear of.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Later {
    Nothing,
    /// A later run changed the file and kept no original of it: every hunk is refused.
    File,
    /// The stretches later runs changed (0-based lines of the file now, end exclusive).
    Stretches(Vec<(usize, usize)>),
}

impl Later {
    fn blocks(&self, hunk: &Hunk, at: Option<usize>) -> bool {
        match self {
            Later::Nothing => false,
            Later::File => true,
            Later::Stretches(stretches) => hunks::touches(hunk, at, stretches),
        }
    }
}

/// One file's hunk review: the diff, each hunk's state and the file's progress.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileHunks {
    pub changes: DiffResult,
    pub hunks: Vec<HunkState>,
    pub progress: Progress,
}

/// The hunk review of a file whose base is `base` and whose text now is `current`; None when there is no diff (over the caps). A hunk
/// cut short by the diff's cap cannot be put back (`unreadable`).
pub fn file_hunks(base: &str, current: &str, decisions: &HashMap<String, Decision>, later: &Later) -> Option<FileHunks> {
    let (changes, prints) = hunks::hunks(base, current)?;
    let last = changes.hunks.len().saturating_sub(1);
    let states = changes
        .hunks
        .iter()
        .zip(&prints)
        .enumerate()
        .map(|(index, (hunk, print))| HunkState {
            fingerprint: print.clone(),
            decision: decisions.get(print).copied().filter(|d| *d == Decision::Accepted),
            refusal: if changes.truncated && index == last {
                Some(Refusal::Unreadable)
            } else {
                later.blocks(hunk, None).then_some(Refusal::LaterRun)
            },
        })
        .collect();
    Some(FileHunks { progress: hunks::progress(&prints, decisions), changes, hunks: states })
}

/// `{changes, hunks: [{fingerprint, decision?, refusal?}], handled, total, refusal?}` for the page: `changes` as the 변경사항 view
/// reads it (`ChangeSet`'s JSON with `base`), `refusal` when the file cannot be reviewed now (`missing`, `unreadable`).
pub fn file_hunks_json(review: Result<FileHunks, Refusal>, base: &'static str, run_id: Option<&str>, created: bool) -> Value {
    match review {
        Err(refusal) => json!({"hunks": [], "handled": 0, "total": 0, "refusal": refusal.word()}),
        Ok(review) => {
            let changes = crate::changes::ChangeSet {
                base,
                hunks: review.changes.hunks.clone(),
                added: review.changes.added,
                removed: review.changes.removed,
                truncated: review.changes.truncated,
                unchanged: review.changes.unchanged,
                run_id: run_id.map(str::to_string),
                head_sha: None,
                created: created.then_some(true),
            };
            json!({
                "changes": changes.to_json(),
                "hunks": review.hunks.iter().map(HunkState::to_json).collect::<Vec<_>>(),
                "handled": review.progress.handled,
                "total": review.progress.total,
            })
        }
    }
}

/// Puts back one hunk of the file `path` under `root` whose base is `base` and whose text now is `current`: the hunk named
/// `fingerprint` as the diff reads it now, else `shown` (the hunk as the page showed it) where its new side still stands. The file is
/// read again just before it is written and nothing is written when it changed since. A file that then reads as its base is
/// reverted as a whole (a new one trashed). The decision goes in `book`.
#[allow(clippy::too_many_arguments)]
pub fn discard_in(
    root: &str,
    path: &str,
    base: &Base,
    current: Option<String>,
    later: impl FnOnce(&str) -> Later,
    fingerprint: &str,
    shown: Option<&Hunk>,
    book: Option<&ReviewBook>,
    trash: &dyn Fn(&Path) -> Result<(), String>,
) -> Outcome {
    let Some(file) = regular_file(path, root) else { return Outcome::Refused(Refusal::Missing) };
    let Some(current) = current else { return Outcome::Refused(Refusal::Unreadable) };
    let Some((changes, prints)) = hunks::hunks(&base.text, &current) else { return Outcome::Refused(Refusal::Unreadable) };
    let hunk = match prints.iter().position(|p| p == fingerprint) {
        Some(index) if changes.truncated && index + 1 == changes.hunks.len() => return Outcome::Refused(Refusal::Unreadable),
        Some(index) => changes.hunks[index].clone(),
        None => match shown {
            Some(hunk) => hunk.clone(),
            None => return Outcome::Refused(Refusal::Conflict),
        },
    };
    let Some(at) = hunks::place(&current, &hunk) else { return Outcome::Refused(Refusal::Conflict) };
    if later(&current).blocks(&hunk, Some(at)) {
        return Outcome::Refused(Refusal::LaterRun);
    }
    let Some(next) = hunks::discard(&current, &hunk, &base.text) else { return Outcome::Refused(Refusal::Conflict) };
    let whole = hunks::same_text(&next, &base.text);
    let done = if whole && base.created {
        trash(&file).map(|()| true)
    } else {
        write_if(if whole { &base.text } else { &next }, &file, Some(&current)).map_err(|e| e.to_string())
    };
    match done {
        Ok(true) => {
            if let Some(book) = book {
                let _ = book.record(path, &[fingerprint.to_string()], Decision::Discarded);
            }
            Outcome::Reverted
        }
        Ok(false) => Outcome::Refused(Refusal::Conflict),
        Err(error) => Outcome::Failed(error),
    }
}

/// The run's path as the store spells it, with its original; None when the run kept no original of it under `root`.
fn kept(store: &FileOriginalStore, session_id: &str, run_id: &str, root: &str, path: &str) -> Option<(String, FileOriginal)> {
    let path = store.files(session_id, run_id, root).into_iter().find(|p| same_path(p, path))?;
    let original = store.find(session_id, Some(run_id), &path, Some(root))?;
    Some((path, original))
}

/// The stretches of `path` (as it reads now, `current`) that runs of the pane newer than `run_id` changed: everything after the oldest
/// of them that touched it, from the original it kept (the file as `run_id` and anything before left it); the whole file when it
/// kept none or the diff cannot be read.
fn later_changes(store: &FileOriginalStore, session_id: &str, run_id: &str, root: &str, path: &str, current: &str) -> Later {
    let runs = store.runs(session_id);
    let Some(index) = runs.iter().position(|r| r == run_id) else { return Later::Nothing };
    let Some(next) = runs[..index].iter().rev().find(|r| store.paths(session_id, r, root).iter().any(|p| same_path(p, path))) else {
        return Later::Nothing;
    };
    let Some(before) = store.find(session_id, Some(next), path, Some(root)) else { return Later::File };
    match diff::compute(&before.text, current) {
        Some(changes) if !changes.truncated => Later::Stretches(hunks::changed_stretches(&changes.hunks)),
        _ => Later::File,
    }
}

/// The hunk review of one file `run_id` (a notice run: the run it came from) of the pane changed (`runChangesHunks`); None when the
/// run kept no original of it under `root`.
pub fn hunk_review(store: &FileOriginalStore, session_id: &str, run_id: &str, root: &str, path: &str) -> Option<Value> {
    let run_id = source_run(run_id);
    let (path, original) = kept(store, session_id, run_id, root, path)?;
    let review = (|| {
        regular_file(&path, root).ok_or(Refusal::Missing)?;
        let current = store.current(&path, Path::new(root)).ok_or(Refusal::Unreadable)?;
        let decisions = store.review_book(session_id, run_id, root).map(|b| b.decisions(&path)).unwrap_or_default();
        let later = later_changes(store, session_id, run_id, root, &path, &current);
        file_hunks(&original.text, &current, &decisions, &later).ok_or(Refusal::Unreadable)
    })();
    Some(file_hunks_json(review, "run", Some(&original.run_id), original.created))
}

/// Puts back one hunk of a file `run_id` changed ([`discard_in`] with the run's original as the base and its later runs' stretches
/// kept clear of); refused as `missing` when the run kept no original of it.
#[allow(clippy::too_many_arguments)]
pub fn discard_hunk(
    store: &FileOriginalStore,
    session_id: &str,
    run_id: &str,
    root: &str,
    path: &str,
    fingerprint: &str,
    shown: Option<&Hunk>,
    trash: &dyn Fn(&Path) -> Result<(), String>,
) -> Outcome {
    let run_id = source_run(run_id);
    let Some((path, original)) = kept(store, session_id, run_id, root, path) else { return Outcome::Refused(Refusal::Missing) };
    let base = Base { text: original.text, created: original.created };
    let book = store.review_book(session_id, run_id, root);
    let current = store.current(&path, Path::new(root));
    let later = |now: &str| later_changes(store, session_id, run_id, root, &path, now);
    discard_in(root, &path, &base, current, later, fingerprint, shown, book.as_ref(), trash)
}

/// Records the hunks of `paths` as taken (받기; nothing is written to the files): those named by `fingerprints`, else every hunk each
/// file has now (남은 부분 모두 받기). False when the run has no book under `root` or it could not be written.
pub fn accept_hunks(
    store: &FileOriginalStore,
    session_id: &str,
    run_id: &str,
    root: &str,
    paths: &[String],
    fingerprints: Option<&[String]>,
) -> bool {
    let run_id = source_run(run_id);
    let Some(book) = store.review_book(session_id, run_id, root) else { return false };
    let mut written = true;
    for wanted in paths {
        let Some((path, original)) = kept(store, session_id, run_id, root, wanted) else { continue };
        let prints = match fingerprints {
            Some(given) => given.to_vec(),
            None => match store.current(&path, Path::new(root)).and_then(|now| hunks::hunks(&original.text, &now)) {
                Some((_, prints)) => prints,
                None => continue,
            },
        };
        if !prints.is_empty() {
            written &= book.record(&path, &prints, Decision::Accepted).is_ok();
        }
    }
    written
}

/// Maximum file size read during revert (matches [`diff::MAXIMUM_SIDE_BYTES`]).
const MAXIMUM_BYTES: u64 = diff::MAXIMUM_SIDE_BYTES as u64;

#[cfg(windows)]
pub(crate) fn is_reparse_point(meta: &std::fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt as _;
    meta.file_attributes() & 0x0400 != 0
}

#[cfg(not(windows))]
pub(crate) fn is_reparse_point(_meta: &std::fs::Metadata) -> bool {
    false
}

#[cfg(unix)]
pub(crate) fn same_file(expected: &std::fs::Metadata, opened: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt as _;
    expected.dev() == opened.dev() && expected.ino() == opened.ino()
}

#[cfg(not(unix))]
pub(crate) fn same_file(_expected: &std::fs::Metadata, _opened: &std::fs::Metadata) -> bool {
    true
}

/// The real path of the regular file at `relative` inside `root`; None when it is gone, a link, a folder or leaves the root.
pub fn regular_file(relative: &str, root: &str) -> Option<PathBuf> {
    let real = workspace_files::resolve(relative, Path::new(root))?;
    let meta = std::fs::symlink_metadata(&real).ok()?;
    (!meta.file_type().is_symlink() && !is_reparse_point(&meta) && meta.file_type().is_file()).then_some(real)
}

fn item(
    store: &FileOriginalStore,
    path: &str,
    original: &FileOriginal,
    root: &str,
    touched_later: bool,
    decisions: &HashMap<String, Decision>,
) -> Item {
    let action = if original.created { Action::Trash } else { Action::Restore };
    let gone = hunks::progress(&[], decisions);
    let refused = |refusal| Item { path: path.to_string(), action, restored: 0, removed: 0, refusal: Some(refusal), progress: gone };
    if regular_file(path, root).is_none() {
        return refused(Refusal::Missing);
    }
    let Some(current) = store.current(path, Path::new(root)) else { return refused(Refusal::Unreadable) };
    let reviewed = hunks::hunks(&original.text, &current);
    let progress = reviewed.as_ref().map_or(gone, |(_, prints)| hunks::progress(prints, decisions));
    let changes = reviewed.map(|(changes, _)| changes);
    let (restored, removed) = changes.as_ref().map_or((0, 0), |c| (c.removed, c.added));
    let refusal = if touched_later {
        Some(Refusal::LaterRun)
    } else if !original.created && current == original.text {
        return refused(Refusal::Unchanged);
    } else {
        None
    };
    Item { path: path.to_string(), action, restored, removed, refusal, progress }
}

/// Paths a run of the pane newer than `run_id` changed too (kept or refused: either way that run's work is in the file).
fn later_paths(store: &FileOriginalStore, session_id: &str, run_id: &str, root: &str) -> Vec<String> {
    let runs = store.runs(session_id);
    let Some(index) = runs.iter().position(|r| r == run_id) else { return vec![] };
    runs[..index].iter().flat_map(|r| store.paths(session_id, r, root)).collect()
}

/// `text` in `encoding`, its byte order mark first; None when the encoding cannot hold it (CP949 and a character it has no code for).
pub(crate) fn encode(text: &str, encoding: TextEncodingKind) -> Option<Vec<u8>> {
    let mut out = byte_order_mark(encoding).to_vec();
    match encoding {
        TextEncodingKind::Utf8 | TextEncodingKind::Utf8Bom => out.extend_from_slice(text.as_bytes()),
        TextEncodingKind::Utf16Le => out.extend(text.encode_utf16().flat_map(u16::to_le_bytes)),
        TextEncodingKind::Utf16Be => out.extend(text.encode_utf16().flat_map(u16::to_be_bytes)),
        TextEncodingKind::Utf32Le => out.extend(text.chars().flat_map(|c| (c as u32).to_le_bytes())),
        TextEncodingKind::Utf32Be => out.extend(text.chars().flat_map(|c| (c as u32).to_be_bytes())),
        TextEncodingKind::Cp949 => {
            let (bytes, _, unmappable) = encoding_rs::EUC_KR.encode(text);
            if unmappable {
                return None;
            }
            out.extend_from_slice(&bytes);
        }
    }
    Some(out)
}

/// Writes `text` in place of the file at `path` in the encoding the file is in now, keeping its permissions: to a temporary name beside
/// it, then moved over. Refused, the file untouched, when `path` is a symlink/non-regular file, exceeds [`MAXIMUM_BYTES`], or when the
/// bytes would not read back as `text`. With `expected`, nothing is written either (`Ok(false)`) when the file no longer reads as it.
pub(crate) fn write_if(text: &str, path: &Path, expected: Option<&str>) -> std::io::Result<bool> {
    let meta = std::fs::symlink_metadata(path)?;
    if meta.file_type().is_symlink() || is_reparse_point(&meta) || !meta.file_type().is_file() {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "revert target is not a regular file"));
    }
    if meta.len() > MAXIMUM_BYTES {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "revert target exceeds maximum size"));
    }
    let permissions = meta.permissions();
    let file = std::fs::File::open(path)?;
    let opened_meta = file.metadata()?;
    if !opened_meta.file_type().is_file() || !same_file(&meta, &opened_meta) {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "revert target changed before read"));
    }
    let mut current = Vec::new();
    file.take(MAXIMUM_BYTES + 1).read_to_end(&mut current)?;
    if current.len() as u64 > MAXIMUM_BYTES {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "revert target exceeds maximum size"));
    }
    let decoded = decode_text(&current, false);
    if let Some(expected) = expected
        && decoded.as_ref().is_none_or(|d| d.text != expected)
    {
        return Ok(false);
    }
    let encoding = decoded.map_or(TextEncodingKind::Utf8, |d| d.encoding);
    let refused = || std::io::Error::new(std::io::ErrorKind::InvalidData, "the original does not fit the file's encoding");
    let bytes = encode(text, encoding).ok_or_else(refused)?;
    // An empty original reads back as nothing whatever the encoding; anything else must read back as itself.
    if !text.is_empty() && decode_text(&bytes, false).is_none_or(|d| d.text != text) {
        return Err(refused());
    }
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let temporary = path.with_file_name(format!(".{name}.{}.revert.tmp", uuid::Uuid::new_v4().simple()));
    let written = (|| -> std::io::Result<()> {
        let mut temp_file = std::fs::OpenOptions::new().write(true).create_new(true).open(&temporary)?;
        temp_file.write_all(&bytes)?;
        temp_file.sync_all()?;
        drop(temp_file);
        std::fs::set_permissions(&temporary, permissions)?;
        std::fs::rename(&temporary, path)
    })();
    if written.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    written.map(|()| true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_encoding_reads_back_as_the_text() {
        let text = "한글 line\r\nnext\n";
        for kind in [
            TextEncodingKind::Utf8,
            TextEncodingKind::Utf8Bom,
            TextEncodingKind::Utf16Le,
            TextEncodingKind::Utf16Be,
            TextEncodingKind::Utf32Le,
            TextEncodingKind::Utf32Be,
            TextEncodingKind::Cp949,
        ] {
            let bytes = encode(text, kind).expect("encodes");
            let decoded = decode_text(&bytes, false).expect("decodes");
            assert_eq!(decoded.text, text, "{kind:?}");
        }
        assert!(encode("emoji 😀", TextEncodingKind::Cp949).is_none(), "CP949 has no code for it");
    }

    /// P1-SEC-2: `write` refuses symlinks and files exceeding `MAXIMUM_BYTES` without mutating targets.
    #[test]
    fn write_rejects_symlinks_and_oversized_files() {
        let base = std::env::temp_dir().join(format!("mighty-revert-sec2-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&base).unwrap();

        // Oversized target (> 1 MiB) is refused.
        let huge = base.join("huge.txt");
        std::fs::write(&huge, vec![b'a'; (MAXIMUM_BYTES as usize) + 16]).unwrap();
        assert!(write_if("small\n", &huge, None).is_err(), "files exceeding MAXIMUM_BYTES must be rejected");
        assert_eq!(std::fs::metadata(&huge).unwrap().len(), MAXIMUM_BYTES + 16, "oversized file left untouched");

        // Symlink target is refused without touching the pointed-to file.
        #[cfg(unix)]
        {
            let target = base.join("real.txt");
            let link = base.join("link.txt");
            std::fs::write(&target, "secret\n").unwrap();
            std::os::unix::fs::symlink(&target, &link).unwrap();
            assert!(write_if("overwritten\n", &link, None).is_err(), "symlink target must be rejected");
            assert_eq!(std::fs::read_to_string(&target).unwrap(), "secret\n", "symlink target content untouched");
        }

        let _ = std::fs::remove_dir_all(&base);
    }
}
