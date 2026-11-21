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
//! A file the run created is moved to the Trash (the Recycle Bin) through the caller's `trash`, never deleted. A restored file keeps the
//! encoding it is in now (its byte order mark too: the store keeps originals without one) and its permissions; its line endings are the
//! original's, which the store kept as they were.

use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};

use mighty_composer::text::{TextEncodingKind, byte_order_mark, decode_text};
use mighty_core::workspace_files;
use serde_json::{Map, Value, json};

use crate::changes::{FileOriginal, FileOriginalStore, same_path, source_run};
use crate::diff;

/// Why a path cannot be reverted now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    LaterRun,
    Missing,
    Unreadable,
    Unchanged,
}

impl Refusal {
    pub fn word(self) -> &'static str {
        match self {
            Refusal::LaterRun => "laterRun",
            Refusal::Missing => "missing",
            Refusal::Unreadable => "unreadable",
            Refusal::Unchanged => "unchanged",
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
}

impl Item {
    pub fn revertible(&self) -> bool {
        self.refusal.is_none()
    }

    /// `{path, action, restored, removed, refusal?}` for the page.
    pub fn to_json(&self) -> Value {
        let mut out = Map::new();
        out.insert("path".into(), json!(self.path));
        out.insert("action".into(), json!(if self.action == Action::Trash { "trash" } else { "restore" }));
        out.insert("restored".into(), json!(self.restored));
        out.insert("removed".into(), json!(self.removed));
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
    store
        .files(session_id, run_id, root)
        .into_iter()
        .filter_map(|path| {
            let original = store.find(session_id, Some(run_id), &path, Some(root))?;
            Some(item(store, &path, &original, root, later.iter().any(|p| same_path(p, &path))))
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
        let planned = item(store, &path, &original, root, later.iter().any(|p| same_path(p, &path)));
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
            Action::Restore => write(&original.text, &file).map_err(|e| e.to_string()),
        };
        outcomes.push((path, done.map_or_else(Outcome::Failed, |()| Outcome::Reverted)));
    }
    outcomes
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
fn regular_file(relative: &str, root: &str) -> Option<PathBuf> {
    let real = workspace_files::resolve(relative, Path::new(root))?;
    let meta = std::fs::symlink_metadata(&real).ok()?;
    (!meta.file_type().is_symlink() && !is_reparse_point(&meta) && meta.file_type().is_file()).then_some(real)
}

fn item(store: &FileOriginalStore, path: &str, original: &FileOriginal, root: &str, touched_later: bool) -> Item {
    let action = if original.created { Action::Trash } else { Action::Restore };
    let refused = |refusal| Item { path: path.to_string(), action, restored: 0, removed: 0, refusal: Some(refusal) };
    if regular_file(path, root).is_none() {
        return refused(Refusal::Missing);
    }
    let Some(current) = store.current(path, Path::new(root)) else { return refused(Refusal::Unreadable) };
    let changes = diff::compute(&original.text, &current);
    let (restored, removed) = changes.as_ref().map_or((0, 0), |c| (c.removed, c.added));
    let refusal = if touched_later {
        Some(Refusal::LaterRun)
    } else if !original.created && current == original.text {
        return refused(Refusal::Unchanged);
    } else {
        None
    };
    Item { path: path.to_string(), action, restored, removed, refusal }
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
/// bytes would not read back as `text`.
fn write(text: &str, path: &Path) -> std::io::Result<()> {
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
    let encoding = decode_text(&current, false).map_or(TextEncodingKind::Utf8, |d| d.encoding);
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
    written
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
        assert!(write("small\n", &huge).is_err(), "files exceeding MAXIMUM_BYTES must be rejected");
        assert_eq!(std::fs::metadata(&huge).unwrap().len(), MAXIMUM_BYTES + 16, "oversized file left untouched");

        // Symlink target is refused without touching the pointed-to file.
        #[cfg(unix)]
        {
            let target = base.join("real.txt");
            let link = base.join("link.txt");
            std::fs::write(&target, "secret\n").unwrap();
            std::os::unix::fs::symlink(&target, &link).unwrap();
            assert!(write("overwritten\n", &link).is_err(), "symlink target must be rejected");
            assert_eq!(std::fs::read_to_string(&target).unwrap(), "secret\n", "symlink target content untouched");
        }

        let _ = std::fs::remove_dir_all(&base);
    }
}
