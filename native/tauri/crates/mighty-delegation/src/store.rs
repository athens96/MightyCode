//! `delegation-state.json` (C# Core/DelegationFileStore.cs:90-213, `DelegationFile`, `DelegationFileStore`; M/DelegationFileStore.swift:
//! 71-145): version 1 beside `workspace-state.json`; load (missing: empty; a directory, a link or over 4 MiB: Unreadable; 40 × 50 ms
//! retries) and save (prune first, `<path>.<guid>.tmp` then replace, 40 × 50 ms; 0600 on Unix; no fsync; the caller's `SaveOrKeep`).
//! Owner: W7-A.
//!
//! The load reads what C#'s `JsonSerializer` with the app's wire options (`JsonSerializerDefaults.Web`) and
//! `RespectRequiredConstructorParameters`/`RespectNullableAnnotations` reads: member names without regard to case (the last of the same
//! name wins), numbers also as strings, every required member present, no null where C# has no `?`, an enum only by its exact name,
//! unknown members passed over, and no byte-order mark.

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Serialize;
use serde_json::{Map, Value};

use crate::copy::{DelegationCopy, DelegationCopyKind};
use crate::json;
use crate::model::{
    ChildRecord, ChildState, DeliveryLane, DeliveryReceipt, DeliveryRoute, FollowUp, MergeKind, MergeRecord, Notice, NoticeKind,
};
use crate::request::DelegationName;

/// `DelegationFileStore.FileName`.
pub const FILE_NAME: &str = "delegation-state.json";
/// `DelegationFileStore.MaximumFileBytes`: 4 MiB.
pub const MAXIMUM_FILE_BYTES: usize = 4 * 1024 * 1024;
/// `DelegationFile.CurrentVersion`.
pub const CURRENT_VERSION: i32 = 1;

/// `DelegationFile`: everything the delegation side file holds (M/DelegationFileStore.swift:71-97). It is the authority for each
/// child's parent link and working folder; receipts travel inside their notice or follow-up. A value: every change makes a new file.
/// The file's keys: `children, copies, followUps, merges, notices, version`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DelegationFile {
    pub version: i32,
    pub children: Vec<ChildRecord>,
    pub notices: Vec<Notice>,
    pub follow_ups: Vec<FollowUp>,
    pub merges: Vec<MergeRecord>,
    pub copies: Vec<DelegationCopy>,
}

impl Default for DelegationFile {
    fn default() -> DelegationFile {
        DelegationFile::empty()
    }
}

impl DelegationFile {
    /// `DelegationFile.Empty`.
    pub fn empty() -> DelegationFile {
        DelegationFile { version: CURRENT_VERSION, children: vec![], notices: vec![], follow_ups: vec![], merges: vec![], copies: vec![] }
    }

    /// `DelegationFile.Of`.
    pub fn of(
        children: Vec<ChildRecord>,
        notices: Vec<Notice>,
        follow_ups: Vec<FollowUp>,
        merges: Vec<MergeRecord>,
        copies: Vec<DelegationCopy>,
    ) -> DelegationFile {
        DelegationFile { version: CURRENT_VERSION, children, notices, follow_ups, merges, copies }
    }

    /// `Copy`: the stored copy of one child's TASK.md or REPORT.md.
    pub fn copy(&self, child_id: &str, kind: DelegationCopyKind) -> Option<&DelegationCopy> {
        self.copies.iter().find(|copy| copy.child_id == child_id && copy.kind == kind)
    }

    /// `SettingCopy`: this file with `copy` in place of the child's earlier copy of the same kind (which goes; the new one is last).
    pub fn setting_copy(&self, copy: DelegationCopy) -> DelegationFile {
        let mut copies: Vec<DelegationCopy> =
            self.copies.iter().filter(|old| !(old.child_id == copy.child_id && old.kind == copy.kind)).cloned().collect();
        copies.push(copy);
        DelegationFile { copies, ..self.clone() }
    }

    /// `SettingChild`: this file with the child at `index` replaced.
    pub fn setting_child(&self, index: usize, child: ChildRecord) -> DelegationFile {
        let mut file = self.clone();
        file.children[index] = child;
        file
    }

    /// The index of the child `id`.
    pub fn child_index(&self, id: &str) -> Option<usize> {
        self.children.iter().position(|child| child.id == id)
    }

    /// The child `id`.
    pub fn child(&self, id: &str) -> Option<&ChildRecord> {
        self.children.iter().find(|child| child.id == id)
    }
}

/// `DelegationFileError`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DelegationFileError {
    /// The file would pass [`MAXIMUM_FILE_BYTES`] (the store_full refusal); nothing was written.
    Full,
    /// The file on disk is not a regular file, is over the cap, or is not a delegation file of this version. It is left untouched.
    Unreadable,
}

/// A save the disk or the cap refused (C# `DelegationFileStore.IsSaveFailure`: a `DelegationFileException`, an `IOException` or an
/// `UnauthorizedAccessException`), which callers answer instead of letting it escape.
#[derive(Debug)]
pub enum SaveError {
    /// The cap or the file on disk (`DelegationFileException`).
    File(DelegationFileError),
    /// The disk refused it.
    Io(std::io::Error),
}

impl SaveError {
    /// Whether this is [`DelegationFileError::Full`].
    pub fn is_full(&self) -> bool {
        matches!(self, SaveError::File(DelegationFileError::Full))
    }
}

impl std::fmt::Display for SaveError {
    /// The exception's message: `DelegationFileException`'s own, or the IO error's.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SaveError::File(DelegationFileError::Full) => f.write_str("The delegation file is full."),
            SaveError::File(DelegationFileError::Unreadable) => f.write_str("The delegation file is unreadable."),
            SaveError::Io(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for SaveError {}

impl From<std::io::Error> for SaveError {
    fn from(error: std::io::Error) -> SaveError {
        SaveError::Io(error)
    }
}

/// `DelegationFileStore`: reads and writes `delegation-state.json` beside `workspace-state.json` in the profile folder. A save replaces
/// the file atomically and is refused, writing nothing, when the file would pass 4 MiB. A missing file loads as empty. Another program
/// holding the file open for a moment only delays a replace (or a read): it is tried again `replace_attempts` times 50 ms apart, on
/// the calling thread, because the coordinator's saves must not be interleaved. `refused` hears each refused read before it is tried
/// again.
pub struct DelegationFileStore {
    folder: PathBuf,
    replace_attempts: u32,
    refused: Option<Box<dyn Fn() + Send + Sync>>,
}

impl std::fmt::Debug for DelegationFileStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DelegationFileStore").field("folder", &self.folder).field("replace_attempts", &self.replace_attempts).finish()
    }
}

impl DelegationFileStore {
    /// The store in `folder`, trying a refused replace 40 times.
    pub fn new(folder: impl Into<PathBuf>) -> DelegationFileStore {
        DelegationFileStore { folder: folder.into(), replace_attempts: 40, refused: None }
    }

    /// The store trying a refused replace or read `attempts` times.
    pub fn with_attempts(folder: impl Into<PathBuf>, attempts: u32) -> DelegationFileStore {
        DelegationFileStore { folder: folder.into(), replace_attempts: attempts.max(1), refused: None }
    }

    /// The store, telling `refused` of each refused read before it is tried again.
    pub fn with_refused(mut self, refused: impl Fn() + Send + Sync + 'static) -> DelegationFileStore {
        self.refused = Some(Box::new(refused));
        self
    }

    pub fn folder(&self) -> &Path {
        &self.folder
    }

    /// `FilePath`.
    pub fn file_path(&self) -> PathBuf {
        self.folder.join(FILE_NAME)
    }

    /// `Load`: the file as saved; empty when there is none. A read another program refuses for a moment is tried again as a save's
    /// replace is; any read that still fails, like a file that does not decode, is [`DelegationFileError::Unreadable`].
    pub fn load(&self) -> Result<DelegationFile, DelegationFileError> {
        let path = self.file_path();
        let mut attempt = 1;
        let data = loop {
            match read_regular(&path) {
                Ok(Some(data)) => break data,
                Ok(None) => return Ok(DelegationFile::empty()),
                Err(ReadFailure::Unreadable) => return Err(DelegationFileError::Unreadable),
                Err(ReadFailure::Io) if attempt < self.replace_attempts => {
                    if let Some(refused) = &self.refused {
                        refused();
                    }
                    std::thread::sleep(Duration::from_millis(50));
                    attempt += 1;
                }
                Err(ReadFailure::Io) => return Err(DelegationFileError::Unreadable),
            }
        };
        if data.len() > MAXIMUM_FILE_BYTES {
            return Err(DelegationFileError::Unreadable);
        }
        decode(&data).ok_or(DelegationFileError::Unreadable)
    }

    /// `Save(file)`: refused, writing nothing, over the cap; otherwise the file replaced atomically.
    pub fn save(&self, file: &DelegationFile) -> Result<(), SaveError> {
        let data = encode(file);
        if data.len() > MAXIMUM_FILE_BYTES {
            return Err(SaveError::File(DelegationFileError::Full));
        }
        std::fs::create_dir_all(&self.folder)?;
        let path = self.file_path();
        let mut temporary = path.clone().into_os_string();
        temporary.push(format!(".{}.tmp", mighty_core::wire::new_id()));
        let temporary = PathBuf::from(temporary);
        let result = (|| -> std::io::Result<()> {
            std::fs::write(&temporary, &data)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt as _;
                std::fs::set_permissions(&temporary, std::fs::Permissions::from_mode(0o600))?;
            }
            let mut attempt = 1;
            loop {
                match std::fs::rename(&temporary, &path) {
                    Ok(()) => return Ok(()),
                    Err(_) if attempt < self.replace_attempts => {
                        attempt += 1;
                        std::thread::sleep(Duration::from_millis(50));
                    }
                    Err(error) => return Err(error),
                }
            }
        })();
        if temporary.exists() {
            let _ = std::fs::remove_file(&temporary);
        }
        Ok(result?)
    }

    /// `Save(file, context)`: saves `file`, pruning first when it nears the cap (M/DelegationPruning.swift:118-131) — when it leaves
    /// less than one more child's room, pruning frees that room if it can, and otherwise as much as the file itself needs.
    /// [`DelegationFileError::Full`], writing nothing, when even that is not enough. Returns what was written.
    pub fn save_pruned(
        &self,
        file: &DelegationFile,
        context: &crate::pruning::DelegationPruneContext,
    ) -> Result<DelegationFile, SaveError> {
        let kept = file
            .pruned(context, crate::pruning::new_child_room() as i64)
            .or_else(|| file.pruned(context, 0))
            .ok_or(SaveError::File(DelegationFileError::Full))?;
        self.save(&kept)?;
        Ok(kept)
    }
}

/// `DelegationFileStore.Encode`: the exact bytes a save writes, for measuring what still fits.
pub fn encode<T: Serialize + ?Sized>(value: &T) -> Vec<u8> {
    json::encode(value)
}

enum ReadFailure {
    /// Not a regular file, or not one this store reads: no retry.
    Unreadable,
    /// The disk refused the read for now.
    Io,
}

/// The file's bytes, None when there is none; a directory, a link (or junction) or a file over the cap is Unreadable.
fn read_regular(path: &Path) -> Result<Option<Vec<u8>>, ReadFailure> {
    let meta = match std::fs::symlink_metadata(path) {
        Ok(meta) => meta,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(ReadFailure::Io),
    };
    if meta.is_dir() || is_link(&meta) || meta.len() > MAXIMUM_FILE_BYTES as u64 {
        return Err(ReadFailure::Unreadable);
    }
    match std::fs::read(path) {
        Ok(data) => Ok(Some(data)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(_) => Err(ReadFailure::Io),
    }
}

/// `FileInfo.LinkTarget is not null`: a symbolic link, or on Windows a junction — the name-surrogate reparse points, which std's
/// `is_symlink` reads from the reparse tag there. Any other reparse point (a cloud placeholder, a deduplicated file) is read as a file,
/// as .NET reads it.
fn is_link(meta: &std::fs::Metadata) -> bool {
    meta.file_type().is_symlink()
}

// ── reading the file as C#'s serializer reads it ────────────────────────────────────────────────────────────────────────

/// The delegation file of `data`, or None when C# would refuse it (Unreadable).
pub fn decode(data: &[u8]) -> Option<DelegationFile> {
    // A byte-order mark is no JSON: C#'s span overload refuses it, as here.
    let root: Value = serde_json::from_slice(data).ok()?;
    let fields = Fields::of(&root)?;
    let file = DelegationFile {
        version: fields.int("version")?,
        children: fields.list("children", child)?,
        notices: fields.list("notices", notice)?,
        follow_ups: fields.list("followUps", follow_up)?,
        merges: fields.list("merges", merge)?,
        copies: fields.list("copies", copy)?,
    };
    (file.version == CURRENT_VERSION).then_some(file)
}

/// One JSON object's members as `JsonSerializerDefaults.Web` matches them: without regard to case, the last of a name winning.
struct Fields<'a>(&'a Map<String, Value>);

impl<'a> Fields<'a> {
    fn of(value: &'a Value) -> Option<Fields<'a>> {
        value.as_object().map(Fields)
    }

    /// The member's value; None when it is missing. A JSON null is Some(Null). Names match as C#'s serializer matches them here: every
    /// name is ASCII, and only an ASCII letter's case is ignored (no non-ASCII letter that upper-cases to one, `ſ` or `ı`, stands in for
    /// it — the parity corpus holds `verſion`); of two that match the last wins.
    fn member(&self, name: &str) -> Option<&'a Value> {
        self.0.iter().rev().find(|(key, _)| key.eq_ignore_ascii_case(name)).map(|(_, value)| value)
    }

    /// A required, non-null member.
    fn required(&self, name: &str) -> Option<&'a Value> {
        self.member(name).filter(|value| !value.is_null())
    }

    fn string(&self, name: &str) -> Option<String> {
        self.required(name)?.as_str().map(str::to_string)
    }

    /// An optional string: missing or null is None; anything else must be a string.
    fn optional_string(&self, name: &str) -> Option<Option<String>> {
        match self.member(name) {
            None | Some(Value::Null) => Some(None),
            Some(Value::String(text)) => Some(Some(text.clone())),
            Some(_) => None,
        }
    }

    fn int(&self, name: &str) -> Option<i32> {
        number(self.required(name)?).and_then(|n| i32::try_from(n).ok())
    }

    fn long(&self, name: &str) -> Option<i64> {
        number(self.required(name)?)
    }

    fn boolean(&self, name: &str) -> Option<bool> {
        self.required(name)?.as_bool()
    }

    fn optional_boolean(&self, name: &str) -> Option<Option<bool>> {
        match self.member(name) {
            None | Some(Value::Null) => Some(None),
            Some(Value::Bool(value)) => Some(Some(*value)),
            Some(_) => None,
        }
    }

    fn name<T: DelegationName>(&self, name: &str) -> Option<T> {
        T::parse(self.required(name)?.as_str()?)
    }

    fn optional<T>(&self, name: &str, read: fn(&Value) -> Option<T>) -> Option<Option<T>> {
        match self.member(name) {
            None | Some(Value::Null) => Some(None),
            Some(value) => read(value).map(Some),
        }
    }

    /// A required list whose every element reads (a null element makes the file unreadable).
    fn list<T>(&self, name: &str, read: fn(&Value) -> Option<T>) -> Option<Vec<T>> {
        self.required(name)?.as_array()?.iter().map(read).collect()
    }
}

/// An integer as `JsonNumberHandling.AllowReadingFromString` reads it: a JSON integer, or a string holding one.
fn number(value: &Value) -> Option<i64> {
    match value {
        Value::Number(number) => number.as_i64(),
        Value::String(text) => text.parse::<i64>().ok(),
        _ => None,
    }
}

fn child(value: &Value) -> Option<ChildRecord> {
    let f = Fields::of(value)?;
    Some(ChildRecord {
        id: f.string("id")?,
        parent_session_id: f.string("parentSessionId")?,
        worktree_path: f.string("worktreePath")?,
        branch: f.string("branch")?,
        parent_branch: f.string("parentBranch")?,
        base_commit: f.string("baseCommit")?,
        starting_mode: f.string("startingMode")?,
        request_key: f.string("requestKey")?,
        follow_up_count: f.int("followUpCount")?,
        report_revision: f.int("reportRevision")?,
        state: f.name::<ChildState>("state")?,
        report_head: f.optional_string("reportHead")?,
        parent_checkout: f.optional_string("parentCheckout")?,
        closed_head: f.optional_string("closedHead")?,
        run_id: f.optional_string("runId")?,
        awaited_run_id: f.optional_string("awaitedRunId")?,
        report_digest: f.optional_string("reportDigest")?,
        started_by_human: f.optional_boolean("startedByHuman")?,
        provider: f.optional_string("provider")?,
        report_run_id: f.optional_string("reportRunId")?,
    })
}

fn receipt(value: &Value) -> Option<DeliveryReceipt> {
    let f = Fields::of(value)?;
    Some(DeliveryReceipt { time: f.string("time")?, route: f.name::<DeliveryRoute>("route")?, run_id: f.string("runId")? })
}

fn notice(value: &Value) -> Option<Notice> {
    let f = Fields::of(value)?;
    Some(Notice {
        id: f.string("id")?,
        child_id: f.string("childId")?,
        report_revision: f.int("reportRevision")?,
        kind: f.name::<NoticeKind>("kind")?,
        lane: f.name::<DeliveryLane>("lane")?,
        receipt: f.optional("receipt", receipt)?,
    })
}

fn follow_up(value: &Value) -> Option<FollowUp> {
    let f = Fields::of(value)?;
    Some(FollowUp {
        id: f.string("id")?,
        child_id: f.string("childId")?,
        text: f.string("text")?,
        lane: f.name::<DeliveryLane>("lane")?,
        receipt: f.optional("receipt", receipt)?,
    })
}

fn merge(value: &Value) -> Option<MergeRecord> {
    let f = Fields::of(value)?;
    Some(MergeRecord {
        child_id: f.string("childId")?,
        kind: f.name::<MergeKind>("kind")?,
        parent_branch: f.string("parentBranch")?,
        pre_merge_commit: f.string("preMergeCommit")?,
        merged_commit: f.string("mergedCommit")?,
        child_head: f.string("childHead")?,
    })
}

fn copy(value: &Value) -> Option<DelegationCopy> {
    let f = Fields::of(value)?;
    Some(DelegationCopy {
        child_id: f.string("childId")?,
        kind: f.name::<DelegationCopyKind>("kind")?,
        revision: f.long("revision")?,
        text: f.string("text")?,
        original_bytes: f.long("originalBytes")?,
        truncated: f.boolean("truncated")?,
    })
}
