//! MC-33/MC-40: the files pane of an SSH workspace: a folder's names through `ssh … ls`, a file's first bytes (up to 1 MiB) with its
//! permissions, and (MC-40) saving from the editor — each a short batch-mode ssh over the machine's shared connection, its script
//! under `/bin/sh -c` (mighty-core `ssh::files`). Opening with the default app and showing in the file manager are not offered.
//!
//! **Saving** keeps what the local save keeps (mighty-files `edit::prepare`: the file's encoding, byte order mark and line endings;
//! here also its permission bits) and refuses the same way. The editor's `diskHash` of a remote file is
//! `ssh-<SHA-256>-<cksum CRC>-<size>` ([`signature`]): the SHA-256 compares what was read here, the POSIX `cksum` lets the far side
//! check the file is still what was read without sending it back. A save reads the file again (refused as a conflict when it no longer
//! matches what the editor opened, unless the person chose to overwrite), sends the new bytes through ssh's standard input into a new
//! file beside it, and the far side replaces the file with one `mv` only while its `cksum` is still what was just read. Nothing of the
//! file ever goes on a command line.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use mighty_core::locale::Locale;
use mighty_core::ssh::failure::Failure;
use mighty_core::ssh::files::{self as remote, Ended};
use mighty_files::edit::{SaveOutcome, SaveRefusal};
use mighty_files::listing::{FileEntry, ListError, Listing};
use mighty_files::preview::{self, PreviewData};
use mighty_run::ssh::SshLaunch;

use crate::host::Host;

/// How long a file's open (and the read a save does first) may take.
const READ_LIMIT: Duration = Duration::from_secs(30);
/// How long a write may take (1 MiB over a slow link).
const WRITE_LIMIT: Duration = Duration::from_secs(60);

/// The launch of `workspace_id`'s folder when it is an SSH workspace: None for any other, Err when its machine cannot be reached.
pub fn launch(host: &Host, workspace_id: &str) -> Option<Result<SshLaunch, String>> {
    let workspace = host.view().snapshot.workspace(workspace_id).filter(|w| w.is_ssh()).cloned()?;
    Some(super::launch_for(host, &workspace))
}

/// One remote folder's listing, as the files pane takes a local one.
pub async fn list(host: &Host, launch: &SshLaunch, relative: &str) -> Result<Listing, ListError> {
    let command = remote::list_command(&launch.dir, relative).ok_or(ListError::OutsideRoot)?;
    let (code, stdout) = super::exec(host, launch, command, 4 * 1024 * 1024).await.map_err(|_| ListError::Unreadable)?;
    let text = String::from_utf8_lossy(&stdout);
    if code != 0 && text.trim().is_empty() {
        return Err(ListError::NotDirectory);
    }
    let (entries, truncated) = remote::parse_listing(&text, relative);
    let entries = entries.into_iter().map(|e| FileEntry::new(&e.name, &e.relative_path, e.is_directory)).collect();
    Ok(Listing { entries: mighty_files::listing::sorted(entries), truncated })
}

/// The editor's `diskHash` of a remote file's bytes: `ssh-<SHA-256>-<cksum CRC>-<size>`.
pub fn signature(bytes: &[u8]) -> String {
    let (crc, size) = remote::cksum(bytes);
    format!("ssh-{}-{crc}-{size}", mighty_files::edit::disk_hash(bytes))
}

/// The `cksum` a [`signature`] names; None for anything else (a local hash, an empty one).
fn signed_cksum(signature: &str) -> Option<(u32, u64)> {
    let mut parts = signature.strip_prefix("ssh-")?.split('-');
    let (sha, crc, size) = (parts.next()?, parts.next()?, parts.next()?);
    (sha.len() == 64 && sha.bytes().all(|b| b.is_ascii_hexdigit()) && parts.next().is_none())
        .then(|| Some((crc.parse().ok()?, size.parse().ok()?)))
        .flatten()
}

/// What opening a remote file for editing found.
enum Open {
    File(remote::Opened),
    Gone,
    Failed(Failure),
}

async fn open(host: &Host, launch: &SshLaunch, relative: &str) -> Open {
    let Some(command) = remote::open_command(&launch.dir, relative) else { return Open::Gone };
    match super::exec_input(host, launch, command, None, remote::MAXIMUM_EDIT_BYTES + 64, READ_LIMIT).await {
        Ok((0, stdout)) => remote::parse_opened(stdout).map_or(Open::Gone, Open::File),
        Ok(_) => Open::Gone,
        Err(failure) => Open::Failed(failure),
    }
}

/// The sentence of an ssh failure for `launch`'s machine.
fn failure_words(host: &Host, launch: &SshLaunch, failure: &Failure) -> String {
    let mut text = host.locale.get_with(failure.key(), &[("host", &launch.target.to_string())]);
    if let Failure::Other(line) = failure
        && !line.is_empty()
    {
        text = format!("{text} {line}");
    }
    text
}

/// One remote file's preview (its first bytes, marked cut past the cap); a whole text file also gets the `diskHash` the editor saves
/// against.
pub async fn preview(host: &Host, launch: &SshLaunch, relative: &str, locale: &Locale) -> PreviewData {
    let failed = |key: &str| {
        let mut data = preview::from_bytes(relative, Vec::new(), false, locale);
        data.failure = Some(locale.get(key));
        data
    };
    match open(host, launch, relative).await {
        Open::File(opened) => {
            let hash = signature(&opened.bytes);
            let mut data = preview::from_bytes(relative, opened.bytes, opened.longer, locale);
            if data.text.is_some() && !data.truncated && !opened.longer {
                data.disk_hash = Some(hash);
            }
            data
        }
        Open::Gone => failed("files.preview.missing"),
        Open::Failed(_) => failed("files.preview.failed"),
    }
}

/// What the editor's disk check found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Check {
    Unchanged(String),
    Modified(String),
    Missing,
}

/// The last checks' answers: the editor asks every few seconds, and without a shared connection (Windows) each ask is a new sign-in.
static CHECKS: Mutex<Option<Checks>> = Mutex::new(None);

/// The checks' answers by workspace and path with the hash asked about, with when each was read.
type Checks = HashMap<(String, String), (Instant, Check)>;

/// The editor's disk check of a remote file against `expected` (a [`signature`]): its `cksum` only, the file read again only when
/// that differs. A connection that failed answers unchanged (no false conflict); a check younger than a few seconds (15 without a
/// shared connection) is answered again.
pub async fn check_disk(host: &Host, launch: &SshLaunch, workspace: &str, relative: &str, expected: &str) -> Check {
    let key = (workspace.to_string(), format!("{relative}\n{expected}"));
    let fresh = if launch.mux.shared() { Duration::from_secs(2) } else { Duration::from_secs(15) };
    if let Some((at, answer)) = crate::host::lock(&CHECKS).get_or_insert_with(HashMap::new).get(&key)
        && at.elapsed() < fresh
    {
        return answer.clone();
    }
    let answer = check_now(host, launch, relative, expected).await;
    let mut checks = crate::host::lock(&CHECKS);
    let checks = checks.get_or_insert_with(HashMap::new);
    checks.retain(|_, (at, _)| at.elapsed() < Duration::from_secs(60));
    if checks.len() < 256 {
        checks.insert(key, (Instant::now(), answer.clone()));
    }
    answer
}

async fn check_now(host: &Host, launch: &SshLaunch, relative: &str, expected: &str) -> Check {
    if let Some(wanted) = signed_cksum(expected)
        && let Some(command) = remote::check_command(&launch.dir, relative)
    {
        match super::exec(host, launch, command, 256).await {
            Ok((0, stdout)) if remote::parse_check(&String::from_utf8_lossy(&stdout)) == Some(wanted) => {
                return Check::Unchanged(expected.to_string());
            }
            Ok((0, _)) => {}
            Ok(_) => return Check::Missing,
            Err(_) => return Check::Unchanged(expected.to_string()),
        }
    }
    match open(host, launch, relative).await {
        Open::File(opened) if !opened.longer => {
            let now = signature(&opened.bytes);
            if now == expected { Check::Unchanged(now) } else { Check::Modified(now) }
        }
        Open::File(_) | Open::Gone => Check::Missing,
        Open::Failed(_) => Check::Unchanged(expected.to_string()),
    }
}

/// Saves `text` over the remote file `relative` (the local save's rules: mighty-files `edit::prepare`), refused as a conflict when
/// the file is no longer what `expected` (the editor's `diskHash`) says, unless `force`.
pub async fn save(
    host: &Host,
    launch: &SshLaunch,
    relative: &str,
    text: &str,
    expected: Option<&str>,
    force: bool,
) -> Result<SaveOutcome, SaveRefusal> {
    let opened = match open(host, launch, relative).await {
        Open::File(opened) => opened,
        Open::Gone => return Err(SaveRefusal::Missing),
        Open::Failed(failure) => return Err(SaveRefusal::Io(failure_words(host, launch, &failure))),
    };
    if opened.longer {
        return Err(SaveRefusal::TooLarge);
    }
    let current = signature(&opened.bytes);
    if !force
        && let Some(expected) = expected.filter(|e| !e.is_empty())
        && expected != current
    {
        return Err(SaveRefusal::Conflict { disk_hash: current });
    }
    let prepared = mighty_files::edit::prepare(&opened.bytes, text)?;
    let write = remote::Write {
        relative: relative.to_string(),
        tag: uuid::Uuid::new_v4().simple().to_string(),
        mode: opened.mode,
        expected: Some(remote::cksum(&opened.bytes)),
        written: remote::cksum(&prepared.bytes),
    };
    let command = remote::write_command(&launch.dir, &write).ok_or(SaveRefusal::Missing)?;
    let size = prepared.bytes.len() as u64;
    let saved = signature(&prepared.bytes);
    match super::exec_input(host, launch, command, Some(prepared.bytes), 256, WRITE_LIMIT).await {
        Ok((code, _)) => match Ended::of(code) {
            Ended::Done => {
                crate::host::lock(&CHECKS).get_or_insert_with(HashMap::new).clear();
                Ok(SaveOutcome { disk_hash: saved, size, modified: None, encoding: prepared.encoding, eol: prepared.eol })
            }
            Ended::Changed => {
                let now = match open(host, launch, relative).await {
                    Open::File(opened) => signature(&opened.bytes),
                    _ => "conflict".to_string(),
                };
                Err(SaveRefusal::Conflict { disk_hash: now })
            }
            Ended::FolderGone | Ended::NotAFile => Err(SaveRefusal::Missing),
            Ended::Incomplete | Ended::Failed(_) => Err(SaveRefusal::Io(host.locale.get("files.editor.remoteSaveFailed"))),
        },
        Err(failure) => Err(SaveRefusal::Io(failure_words(host, launch, &failure))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_signature_carries_the_far_sides_cksum() {
        let signed = signature(b"hello\n");
        assert!(signed.starts_with("ssh-"));
        assert_eq!(signed_cksum(&signed), Some(remote::cksum(b"hello\n")));
        assert_eq!(signed_cksum(&mighty_files::edit::disk_hash(b"x")), None, "a local hash is no remote one");
        assert_eq!(signed_cksum("ssh-zz-1-2"), None);
        assert_eq!(signed_cksum(""), None);
    }
}
