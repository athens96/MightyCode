//! MC-40: 요청 변경 검토 and 바뀐 부분별 받기·버리기 for a pane of an SSH workspace. What a request changed is read from the far side's
//! snapshots (mighty-core `ssh::review`: a tree before its CLI started and one after it ended) instead of the local originals: the
//! files it changed are the two trees' difference, each file's original is its blob in the first tree (a file the request made has
//! none and is `created`), and a later request of the pane that changed the file blocks a hunk as `laterRun` with the lines it changed
//! (its own first tree is "the file as this request left it"). [`RemoteReview`] is both [`ReviewOriginals`] and [`ReviewFiles`], so
//! the review's rules are the local ones ([`crate::revert`]'s `*_on`): the same refusals (`missing`, `unreadable`, `unchanged`,
//! `laterRun`, `conflict`), the same hunks and fingerprints.
//!
//! Every read goes in a batch (one ssh for the pane's requests, one for the contents a review needs); a write is mighty-core
//! `ssh::files`' atomic write, made only while the file is still what was read (its `cksum`), else it is a conflict; a file the request
//! made goes to the store's trash over there. The decisions (받기/버리기) are kept here, beside the worktrees' books.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use mighty_composer::text::{TextEncodingKind, decode_text};
use mighty_core::ssh::files::{self as remote_files, Ended};
use mighty_core::ssh::review::{self as snapshots, Got, Read, Request};
use mighty_platform::ProcessHost;
use mighty_run::ssh::SshLaunch;

use crate::changes::{FileOriginal, same_path};
use crate::hunks::{ReviewBook, book_name};
use crate::revert::{ReviewFiles, ReviewOriginals, encode};

/// The far side, as a review reaches it: a remote command (and its standard input) run to its end. Called off the async runtime.
pub trait RemoteExec: Send + Sync {
    /// The exit code and standard output; Err when the machine could not be reached.
    fn exec(&self, command: String, input: Option<Vec<u8>>) -> Result<(i32, Vec<u8>), String>;
}

/// [`RemoteExec`] over the machine's ssh (a batch-mode `ssh -T`, the shared connection where there is one), waited for on `handle`.
pub struct SshExec {
    pub processes: Arc<dyn ProcessHost>,
    pub launch: SshLaunch,
    pub handle: tokio::runtime::Handle,
}

impl RemoteExec for SshExec {
    fn exec(&self, command: String, input: Option<Vec<u8>>) -> Result<(i32, Vec<u8>), String> {
        let request = self.launch.exec_request(command, &std::collections::BTreeMap::new());
        let captured = self
            .handle
            .block_on(mighty_platform::process::capture(self.processes.as_ref(), request, input, Duration::from_secs(60), 96 << 20))
            .map_err(|e| e.to_string())?;
        if captured.exit_code == mighty_core::ssh::command::SSH_FAILED {
            return Err(String::from_utf8_lossy(&captured.stderr).trim().to_string());
        }
        Ok((captured.exit_code, captured.stdout))
    }
}

/// A file of the workspace folder as last read.
#[derive(Debug, Clone)]
enum Current {
    Missing,
    TooLarge,
    File { mode: u32, bytes: Vec<u8> },
}

/// One pane's requests on an SSH machine, as a review reads and writes them.
pub struct RemoteReview {
    exec: Arc<dyn RemoteExec>,
    dir: String,
    prefix: String,
    /// Newest first.
    requests: Vec<Request>,
    books: PathBuf,
    scope: String,
    blobs: Mutex<HashMap<(String, String), Got>>,
    currents: Mutex<HashMap<String, Current>>,
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|p| p.into_inner())
}

/// A file's text as the review reads it (C# `FileChanges.Current`): None when over 1 MiB, binary or undecodable.
fn text(bytes: &[u8]) -> Option<String> {
    (bytes.len() <= crate::diff::MAXIMUM_SIDE_BYTES).then(|| decode_text(bytes, false).map(|d| d.text)).flatten()
}

impl RemoteReview {
    /// The requests of `pane` in `dir` with snapshots; None when it has none (or the machine could not be reached). `books` is the
    /// folder the decisions are kept in, `scope` names the pane there (machine, folder and pane).
    pub fn load(exec: Arc<dyn RemoteExec>, dir: &str, pane: &str, books: PathBuf, scope: String) -> Option<RemoteReview> {
        let command = snapshots::index_script(dir, pane)?;
        let (code, stdout) = exec.exec(command, None).ok()?;
        if code != 0 {
            return None;
        }
        let (prefix, requests) = snapshots::parse_index(&stdout)?;
        Some(RemoteReview {
            exec,
            dir: dir.to_string(),
            prefix,
            requests,
            books,
            scope,
            blobs: Mutex::new(HashMap::new()),
            currents: Mutex::new(HashMap::new()),
        })
    }

    /// The requests that changed something, newest first.
    pub fn changed_runs(&self) -> Vec<String> {
        self.requests.iter().filter(|r| !r.changes.is_empty()).map(|r| r.run.clone()).collect()
    }

    fn request(&self, run: &str) -> Option<&Request> {
        self.requests.iter().find(|r| r.run == run)
    }

    /// Reads at once what a review of `run` needs: each changed file's original and current text, and the originals of the later
    /// requests that changed the same files.
    pub fn prefetch(&self, run: &str) {
        let Some(index) = self.requests.iter().position(|r| r.run == run) else { return };
        let request = &self.requests[index];
        let mut reads = Vec::new();
        for (_, path) in &request.changes {
            reads.push(Read::Blob { tree: request.start.clone(), path: format!("{}{path}", self.prefix) });
            reads.push(Read::Current(path.clone()));
            for later in &self.requests[..index] {
                if later.changes.iter().any(|(_, p)| same_path(p, path)) {
                    reads.push(Read::Blob { tree: later.start.clone(), path: format!("{}{path}", self.prefix) });
                }
            }
        }
        self.fetch(reads);
    }

    /// Reads what the caches do not hold yet, in one remote command (several for a long list).
    fn fetch(&self, reads: Vec<Read>) {
        let wanted: Vec<Read> = {
            let (blobs, currents) = (lock(&self.blobs), lock(&self.currents));
            let mut seen = Vec::new();
            for read in reads {
                let known = match &read {
                    Read::Blob { tree, path } => blobs.contains_key(&(tree.clone(), path.clone())),
                    Read::Current(path) => currents.contains_key(path),
                };
                if !known && !seen.contains(&read) {
                    seen.push(read);
                }
            }
            seen
        };
        for chunk in wanted.chunks(64) {
            let Some(command) = snapshots::read_script(&self.dir, chunk) else { continue };
            let Ok((0, stdout)) = self.exec.exec(command, None) else { continue };
            let Some(got) = snapshots::parse_reads(&stdout, chunk.len()) else { continue };
            for (read, got) in chunk.iter().zip(got) {
                match read {
                    Read::Blob { tree, path } => {
                        lock(&self.blobs).insert((tree.clone(), path.clone()), got);
                    }
                    Read::Current(path) => {
                        let current = match got {
                            Got::None => Current::Missing,
                            Got::TooLarge(_) => Current::TooLarge,
                            Got::Bytes(mode, bytes) => Current::File { mode: mode.unwrap_or(0o644), bytes },
                        };
                        lock(&self.currents).insert(path.clone(), current);
                    }
                }
            }
        }
    }

    fn blob(&self, tree: &str, path: &str) -> Option<Got> {
        let key = (tree.to_string(), format!("{}{path}", self.prefix));
        if !lock(&self.blobs).contains_key(&key) {
            self.fetch(vec![Read::Blob { tree: key.0.clone(), path: key.1.clone() }]);
        }
        lock(&self.blobs).get(&key).cloned()
    }

    fn file(&self, path: &str) -> Option<Current> {
        if !lock(&self.currents).contains_key(path) {
            self.fetch(vec![Read::Current(path.to_string())]);
        }
        lock(&self.currents).get(path).cloned()
    }
}

impl ReviewOriginals for RemoteReview {
    fn files(&self, run_id: &str) -> Vec<String> {
        self.request(run_id).map(|r| r.changes.iter().map(|(_, p)| p.clone()).collect()).unwrap_or_default()
    }

    fn paths(&self, run_id: &str) -> Vec<String> {
        self.files(run_id)
    }

    fn original(&self, run_id: &str, path: &str) -> Option<FileOriginal> {
        let request = self.request(run_id)?;
        let (status, path) = request.changes.iter().find(|(_, p)| same_path(p, path)).cloned()?;
        match self.blob(&request.start, &path)? {
            Got::Bytes(_, bytes) => Some(FileOriginal { run_id: run_id.to_string(), path, text: text(&bytes)?, created: false }),
            Got::None if status == 'A' => Some(FileOriginal { run_id: run_id.to_string(), path, text: String::new(), created: true }),
            _ => None,
        }
    }

    fn runs(&self) -> Vec<String> {
        self.requests.iter().map(|r| r.run.clone()).collect()
    }

    fn book(&self, run_id: &str) -> Option<ReviewBook> {
        self.request(run_id)?;
        Some(ReviewBook::at(self.books.join(book_name(&format!("{}\n{run_id}", self.scope)))))
    }
}

impl ReviewFiles for RemoteReview {
    fn regular(&self, path: &str) -> bool {
        matches!(self.file(path), Some(Current::File { .. } | Current::TooLarge))
    }

    fn current(&self, path: &str) -> Option<String> {
        match self.file(path)? {
            Current::File { bytes, .. } => text(&bytes),
            _ => None,
        }
    }

    fn write_if(&self, path: &str, text: &str, expected: Option<&str>) -> Result<bool, String> {
        write_current(self.exec.as_ref(), &self.dir, &self.currents, self.file(path), path, text, expected)
    }

    fn trash(&self, path: &str) -> Result<(), String> {
        trash_current(self.exec.as_ref(), &self.dir, &self.currents, self.file(path), path)
    }
}

/// [`ReviewFiles::write_if`] over the far side: `text` written in place of `path` (as it was last read, `read`) with the editor's
/// atomic write, only while it is still what was read (its `cksum`), in the encoding it is in, its permissions kept.
fn write_current(
    exec: &dyn RemoteExec,
    dir: &str,
    currents: &Mutex<HashMap<String, Current>>,
    read: Option<Current>,
    path: &str,
    text: &str,
    expected: Option<&str>,
) -> Result<bool, String> {
    let Some(Current::File { mode, bytes }) = read else {
        return Err("revert target is not a regular file".into());
    };
    if bytes.len() > crate::diff::MAXIMUM_SIDE_BYTES {
        return Err("revert target exceeds maximum size".into());
    }
    let decoded = decode_text(&bytes, false);
    if let Some(expected) = expected
        && decoded.as_ref().is_none_or(|d| d.text != expected)
    {
        return Ok(false);
    }
    let encoding = decoded.map_or(TextEncodingKind::Utf8, |d| d.encoding);
    let refused = || "the original does not fit the file's encoding".to_string();
    let new = encode(text, encoding).ok_or_else(refused)?;
    if !text.is_empty() && decode_text(&new, false).is_none_or(|d| d.text != text) {
        return Err(refused());
    }
    let write = remote_files::Write {
        relative: path.to_string(),
        tag: uuid::Uuid::new_v4().simple().to_string(),
        mode,
        expected: Some(remote_files::cksum(&bytes)),
        written: remote_files::cksum(&new),
    };
    let command = remote_files::write_command(dir, &write).ok_or_else(|| "the path cannot be written".to_string())?;
    let (code, _) = exec.exec(command, Some(new.clone()))?;
    match Ended::of(code) {
        Ended::Done => {
            lock(currents).insert(path.to_string(), Current::File { mode, bytes: new });
            Ok(true)
        }
        Ended::Changed => {
            lock(currents).remove(path);
            Ok(false)
        }
        Ended::NotAFile | Ended::FolderGone => Err("revert target is not a regular file".into()),
        Ended::Incomplete | Ended::Failed(_) => Err(format!("the SSH machine did not write the file ({code})")),
    }
}

/// [`ReviewFiles::trash`] over the far side: a file the run made moved into the folder's store's trash over there, only while it is
/// still what was read.
fn trash_current(
    exec: &dyn RemoteExec,
    dir: &str,
    currents: &Mutex<HashMap<String, Current>>,
    read: Option<Current>,
    path: &str,
) -> Result<(), String> {
    let Some(Current::File { bytes, .. }) = read else { return Err("revert target is not a regular file".into()) };
    let tag = uuid::Uuid::new_v4().simple().to_string();
    let command =
        snapshots::trash_script(dir, path, &tag, remote_files::cksum(&bytes)).ok_or_else(|| "the path cannot be moved".to_string())?;
    let (code, _) = exec.exec(command, None)?;
    lock(currents).remove(path);
    match code {
        0 => {
            lock(currents).insert(path.to_string(), Current::Missing);
            Ok(())
        }
        7 => Err("the file changed on the SSH machine".into()),
        other => Err(format!("the SSH machine did not move the file ({other})")),
    }
}

/// MC-47: a folder on an SSH machine as a hunk review reads and writes it, with no request snapshots: a worktree's files against the
/// commit it shares with its base (MC-30's 바뀐 부분별 받기·버리기 for an SSH pane's worktree). The same reads, atomic writes and
/// trash as [`RemoteReview`]'s.
pub struct RemoteFolder {
    exec: Arc<dyn RemoteExec>,
    dir: String,
    currents: Mutex<HashMap<String, Current>>,
}

impl RemoteFolder {
    pub fn new(exec: Arc<dyn RemoteExec>, dir: &str) -> RemoteFolder {
        RemoteFolder { exec, dir: dir.to_string(), currents: Mutex::new(HashMap::new()) }
    }

    fn file(&self, path: &str) -> Option<Current> {
        if !lock(&self.currents).contains_key(path) {
            let command = mighty_core::ssh::worktree::current_script(&self.dir, &[path.to_string()])?;
            let Ok((0, stdout)) = self.exec.exec(command, None) else { return None };
            let got = snapshots::parse_reads(&stdout, 1)?.pop()?;
            let current = match got {
                Got::None => Current::Missing,
                Got::TooLarge(_) => Current::TooLarge,
                Got::Bytes(mode, bytes) => Current::File { mode: mode.unwrap_or(0o644), bytes },
            };
            lock(&self.currents).insert(path.to_string(), current);
        }
        lock(&self.currents).get(path).cloned()
    }

    /// The file's bytes now; None when it is no regular file or is too large.
    pub fn bytes(&self, path: &str) -> Option<Vec<u8>> {
        match self.file(path)? {
            Current::File { bytes, .. } => Some(bytes),
            _ => None,
        }
    }

    /// Forgets what was read, so the next question reads the far side again.
    pub fn forget(&self) {
        lock(&self.currents).clear();
    }
}

impl ReviewFiles for RemoteFolder {
    fn regular(&self, path: &str) -> bool {
        matches!(self.file(path), Some(Current::File { .. } | Current::TooLarge))
    }

    fn current(&self, path: &str) -> Option<String> {
        match self.file(path)? {
            Current::File { bytes, .. } => text(&bytes),
            _ => None,
        }
    }

    fn write_if(&self, path: &str, text: &str, expected: Option<&str>) -> Result<bool, String> {
        write_current(self.exec.as_ref(), &self.dir, &self.currents, self.file(path), path, text, expected)
    }

    fn trash(&self, path: &str) -> Result<(), String> {
        trash_current(self.exec.as_ref(), &self.dir, &self.currents, self.file(path), path)
    }
}
