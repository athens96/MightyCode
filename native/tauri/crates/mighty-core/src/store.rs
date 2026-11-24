//! Where the state lives, how it loads and how it is saved: C# `StateStore` (MightyCode.Core/StateStore.cs), its `SaveSchedule` and
//! the save timing of `DesktopService` (`QueueSave`, `ScheduleSave`).
//!
//! - Load (`LoadAsync`): the pre-rename profile's file copied in once (default profile only), a file holding a NUL byte (what a power
//!   cut leaves) moved aside to `.unreadable-<time>-<id>` with its `.bak` restored (or an empty state) and a notice in the user's
//!   language; any other file that does not read stops loading and every save, so it is never overwritten.
//! - Save (`SaveAsync`): normalized, every workspace one the load or the user approved, files panes left out, at most 8 MiB, then
//!   written whole or not at all ([`atomic_write`]): a temporary file flushed to the disk, then swapped in with the old file kept
//!   as `.bak` (`ReplaceFileW` on Windows, retried 40 times 50 ms apart).
//!
//! The host decides whether a store may touch the disk at all (`writes`): without it, a load reports what C# would have done (the
//! damaged file is not moved, the legacy one not copied) and a save writes nothing.

use std::fmt;
use std::fs;
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};
use std::time::Duration;

use indexmap::IndexMap;
use serde_json::{Map, Value};

use crate::json::{self, JsonOut, Wire};
use crate::locale::Locale;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use crate::graph::MightyGraphRun;
use crate::model::{
    AppSnapshot, BackgroundWork, LogEntry, PlanRecord, RemoteWorkspaceReference, RunSession, ShellBlockRun, TodoProgress, Workspace,
};
use crate::normalize;
use crate::wire::{self, new_id};

pub const STATE_FILE: &str = "workspace-state.json";
/// The largest state file C# reads or writes (`store.error.fileTooLarge`, `store.error.stateTooLarge`).
pub const MAXIMUM_STATE_BYTES: u64 = 8 * 1024 * 1024;
/// The only `remote` reference version a workspace may carry (C# `RemoteWorkspaceReference.CurrentVersion`).
pub const REMOTE_VERSION: f64 = 2.0;

/// The default profile, `%APPDATA%\MightyCode` on Windows (the platform's per-user config folder elsewhere).
pub fn default_data_dir() -> Option<PathBuf> {
    dirs::config_dir().map(|dir| dir.join("MightyCode"))
}

/// The profile of the app before its rename whose state C# copies once into a default profile that has none: the first of
/// `MightyClaude`, `mighty-claude` holding a state file (C# `MainWindow` ctor). An explicit `--profile` never reads it.
pub fn legacy_data_dir() -> Option<PathBuf> {
    let config = dirs::config_dir()?;
    ["MightyClaude", "mighty-claude"].iter().map(|name| config.join(name)).find(|path| path.join(STATE_FILE).exists())
}

/// Why a state file does not load or save. Each names the C# outcome.
#[derive(Debug)]
pub enum StateError {
    Io(io::Error),
    /// `store.error.fileTooLarge`.
    TooLarge(u64),
    /// `store.error.legacyFileTooLarge`.
    LegacyTooLarge(u64),
    /// `store.error.invalidFormat`: not a version 1 object with `workspaces` and `sessions` arrays.
    InvalidFormat(String),
    /// A `JsonException`: not JSON, or fields that do not decode as the C# records.
    Json(String),
    /// Another exception C# throws on the way (a null it dereferences while it normalizes): the load fails, and a backup's read is
    /// not caught either.
    Crash(String),
    /// `store.error.notLoaded`: the load failed (or never ran); nothing is saved.
    NotLoaded,
    /// `store.error.pathNotApproved`.
    PathNotApproved,
    /// `store.error.stateTooLarge`.
    StateTooLarge(usize),
    /// The host keeps writing off.
    WritesOff,
}

impl StateError {
    /// The C# outcome's name: the locale key of its message, or `json` for a `JsonException`.
    pub fn kind(&self) -> &'static str {
        match self {
            StateError::Io(_) => "IOException",
            StateError::TooLarge(_) => "store.error.fileTooLarge",
            StateError::LegacyTooLarge(_) => "store.error.legacyFileTooLarge",
            StateError::InvalidFormat(_) => "store.error.invalidFormat",
            StateError::Json(_) => "json",
            StateError::Crash(_) => "crash",
            StateError::NotLoaded => "store.error.notLoaded",
            StateError::PathNotApproved => "store.error.pathNotApproved",
            StateError::StateTooLarge(_) => "store.error.stateTooLarge",
            StateError::WritesOff => "writesOff",
        }
    }

    /// The message the window shows (C#'s `ex.Message`; a `JsonException`'s own .NET wording is not reproduced).
    pub fn message(&self, locale: &Locale) -> String {
        match self {
            StateError::Io(e) => e.to_string(),
            StateError::Json(why) | StateError::Crash(why) => why.clone(),
            StateError::WritesOff => "state writing is off".into(),
            other => locale.get(other.kind()),
        }
    }
}

impl fmt::Display for StateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StateError::Io(e) => write!(f, "{e}"),
            StateError::TooLarge(n) | StateError::LegacyTooLarge(n) => {
                write!(f, "the state file is {n} bytes, over the {MAXIMUM_STATE_BYTES}-byte limit")
            }
            StateError::InvalidFormat(why) => write!(f, "the state file is not a version 1 state: {why}"),
            StateError::Json(why) => write!(f, "the state does not decode: {why}"),
            StateError::Crash(why) => write!(f, "the state does not normalize: {why}"),
            StateError::NotLoaded => f.write_str("the state did not load, so it is not saved"),
            StateError::PathNotApproved => f.write_str("a workspace folder was not approved"),
            StateError::StateTooLarge(n) => write!(f, "the state is {n} bytes, over the {MAXIMUM_STATE_BYTES}-byte limit"),
            StateError::WritesOff => f.write_str("state writing is off"),
        }
    }
}

impl std::error::Error for StateError {}

impl From<io::Error> for StateError {
    fn from(e: io::Error) -> Self {
        StateError::Io(e)
    }
}

/// True when the bytes hold a NUL, which the JSON this app writes never does: what a power cut leaves behind (C# `Damaged`).
pub fn damaged(bytes: &[u8]) -> bool {
    bytes.contains(&0)
}

/// Whether a saved workspace object is kept on load (C# `StateStore.KeepsWorkspace`): a local one (no `remote`, or an explicit
/// null), or a remote one whose reference carries version 2 as a JSON number (2.0 included).
pub fn keeps_workspace(value: &Value) -> bool {
    let Some(object) = value.as_object() else { return false };
    match object.get("remote") {
        None | Some(Value::Null) => true,
        Some(Value::Object(remote)) => remote.get("version").filter(|v| v.is_number()).and_then(Value::as_f64) == Some(REMOTE_VERSION),
        Some(_) => false,
    }
}

/// The saved state with only the workspaces [`keeps_workspace`] keeps (C# `KeptWorkspaces`): a key spelled like `workspaces` or
/// `remote` in another case is left out; a kept remote reference's version is written as the integer 2, and a remote workspace
/// that does not decode as a `Workspace` is left out on its own.
pub fn kept_workspaces(root: &Map<String, Value>) -> Map<String, Value> {
    let mut out = Map::new();
    for (name, value) in root {
        if !json::same_ignoring_case(name, "workspaces") {
            out.insert(name.clone(), value.clone());
            continue;
        }
        let (true, Some(list)) = (name == "workspaces", value.as_array()) else { continue };
        let kept: Vec<Value> = list.iter().filter(|w| keeps_workspace(w)).filter_map(kept_workspace).collect();
        out.insert(name.clone(), Value::Array(kept));
    }
    out
}

fn kept_workspace(workspace: &Value) -> Option<Value> {
    let object = workspace.as_object()?;
    let mut item = Map::new();
    let mut remote = false;
    let mut current = true;
    for (name, field) in object {
        if !json::same_ignoring_case(name, "remote") {
            item.insert(name.clone(), field.clone());
        } else if name != "remote" {
            continue;
        } else if let Value::Object(reference) = field {
            remote = true;
            let mut versioned = false;
            let mut kept = Map::new();
            for (part, part_value) in reference {
                if !json::same_ignoring_case(part, "version") {
                    kept.insert(part.clone(), part_value.clone());
                } else if part == "version" && part_value.is_number() && part_value.as_f64() == Some(REMOTE_VERSION) {
                    kept.insert(part.clone(), Value::from(2));
                    versioned = true;
                } else {
                    current = false;
                }
            }
            current &= versioned;
            item.insert(name.clone(), Value::Object(kept));
        } else {
            remote |= !field.is_null();
            item.insert(name.clone(), field.clone());
        }
    }
    let item = Value::Object(item);
    if remote && (!current || Workspace::read(&item).is_err()) {
        return None;
    }
    Some(item)
}

/// A file's text as `File.ReadAllTextAsync` decodes it: a UTF-8 byte order mark skipped, invalid UTF-8 read as U+FFFD.
fn text_of(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(bytes)).into_owned()
}

/// A JSON document as `JsonDocument.Parse` reads one (no comments, no trailing commas, at most 64 levels deep).
pub fn parse_document(text: &str) -> Result<Value, StateError> {
    let value: Value = serde_json::from_str(text).map_err(|e| {
        // A lone surrogate escape: JsonDocument reads it, and the string's read then throws an InvalidOperationException (where
        // the string is an unknown field's, C# skips it and loads: that one case is not reproduced).
        match without_lone_surrogates(text) {
            Some(fixed) if serde_json::from_str::<Value>(&fixed).is_ok() => StateError::Crash(e.to_string()),
            _ => StateError::Json(e.to_string()),
        }
    })?;
    if json::depth(&value) > json::MAXIMUM_DEPTH {
        return Err(StateError::Json("deeper than 64 levels".into()));
    }
    Ok(value)
}

/// The JSON text with every `\uXXXX` escape of a surrogate that has no partner made `�`, or None when it has none.
fn without_lone_surrogates(text: &str) -> Option<String> {
    let b = text.as_bytes();
    let unit = |at: usize| -> Option<u16> {
        (b.get(at) == Some(&b'\\') && b.get(at + 1) == Some(&b'u')).then_some(())?;
        u16::from_str_radix(text.get(at + 2..at + 6)?, 16).ok()
    };
    let mut out = String::with_capacity(text.len());
    let mut changed = false;
    let mut at = 0;
    let mut kept = 0;
    while at < b.len() {
        if b[at] != b'\\' {
            at += 1;
            continue;
        }
        match unit(at) {
            Some(high @ 0xD800..=0xDBFF) if unit(at + 6).is_some_and(|low| (0xDC00..=0xDFFF).contains(&low)) => {
                let _ = high;
                at += 12;
            }
            Some(0xD800..=0xDFFF) => {
                out.push_str(&text[kept..at]);
                out.push_str("\\uFFFD");
                changed = true;
                at += 6;
                kept = at;
            }
            _ => at += 2,
        }
    }
    out.push_str(&text[kept..]);
    changed.then_some(out)
}

/// Decodes a state object as it is, without the load's checks or normalize (C# `JsonSerializer.Deserialize<AppSnapshot>`).
pub fn decode(value: &Value) -> Result<AppSnapshot, StateError> {
    AppSnapshot::read(value).map_err(state_error)
}

fn state_error(e: json::ReadError) -> StateError {
    if e.is_crash() { StateError::Crash(e.0) } else { StateError::Json(e.0) }
}

/// `Wire.Clone`: the state written and read back.
pub fn clone_state(snapshot: &AppSnapshot) -> AppSnapshot {
    let text = snapshot.to_json();
    serde_json::from_str::<Value>(&text).ok().and_then(|v| AppSnapshot::read(&v).ok()).unwrap_or_else(|| snapshot.clone())
}

/// One saved state file's bytes as C# `ReadAsync` reads them: checked (8 MB, version 1 as an integer, `workspaces` and `sessions`
/// arrays), the kept workspaces decoded, `gemini` panes migrated once ([`normalize::antigravity_migrated`], its notes in
/// `locale`), normalized as a restore.
pub fn read_state(bytes: &[u8], locale: &Locale) -> Result<AppSnapshot, StateError> {
    if bytes.len() as u64 > MAXIMUM_STATE_BYTES {
        return Err(StateError::TooLarge(bytes.len() as u64));
    }
    read_state_value(parse_document(&text_of(bytes))?, locale)
}

/// [`read_state`] of a state already parsed (the split state's index with its panes' bodies laid on).
fn read_state_value(value: Value, locale: &Locale) -> Result<AppSnapshot, StateError> {
    let Value::Object(root) = &value else { return Err(StateError::InvalidFormat("not an object".into())) };
    // C# TryGetInt32: an integer literal only (1.0 and 1e0 read as floats here, which as_i64 refuses); on anything but a number
    // it throws (InvalidOperationException).
    if root.get("version").is_some_and(|v| !v.is_number()) {
        return Err(StateError::Crash("version is not a number".into()));
    }
    if root.get("version").and_then(Value::as_i64) != Some(1) {
        return Err(StateError::InvalidFormat("version is not 1".into()));
    }
    if !root.get("workspaces").is_some_and(Value::is_array) || !root.get("sessions").is_some_and(Value::is_array) {
        return Err(StateError::InvalidFormat("workspaces or sessions is not an array".into()));
    }
    let mut restored = decode(&Value::Object(kept_workspaces(root)))?;
    if restored.antigravity_migrated != Some(true) {
        let migrated = restored.sessions().iter().map(|s| std::sync::Arc::new(normalize::antigravity_migrated(s, locale))).collect();
        restored.sessions = Some(migrated);
        restored.antigravity_migrated = Some(true);
    }
    normalize::normalize(&restored, true).map_err(state_error)
}

/// The bytes C# `SaveAsync` writes for a state: normalized, approved, without files panes, at most 8 MiB.
pub fn encode_for_save(snapshot: &AppSnapshot) -> Result<(AppSnapshot, Vec<u8>), StateError> {
    // The remote mirror's workspaces and panes are never written, whoever encodes the state.
    let normalized = normalize::normalize(&snapshot.without_mirrors(), false).map_err(state_error)?;
    let encoded = encode_fitting(&normalized)?;
    Ok((normalized, encoded))
}

/// The state as the one file C# writes (no files panes, every pane whole): what the split index and bodies together hold, which the
/// parity check compares with C#'s save.
pub fn encode_whole(snapshot: &AppSnapshot) -> Vec<u8> {
    encode(snapshot)
}

/// The state's file bytes.
fn encode(snapshot: &AppSnapshot) -> Vec<u8> {
    let mut out = JsonOut::new();
    normalize::stored(snapshot).write(&mut out);
    out.finish().into_bytes()
}

/// The saved diagrams' budget per pane, in the order a state too large for the file tries them (the live one is
/// [`normalize::LIVE_HISTORY_LIMIT`]), and how many log entries a pane keeps after that.
const FITTING_GRAPH_BUDGETS: [i64; 6] = [1024 * 1024, 512 * 1024, 256 * 1024, 128 * 1024, 32 * 1024, 0];
const FITTING_LOG_ENTRIES: [usize; 3] = [200, 100, 40];

/// The bytes written for `normalized`, at most [`MAXIMUM_STATE_BYTES`]. Each pane may keep 2 MiB of diagrams, so a few long-lived
/// panes together outgrow the file; rather than refuse every save from then on (and lose what changed since), the written copy of
/// each pane's diagrams is cut to its newest requests under a smaller and smaller budget, then its log to its newest entries, until
/// it fits. Only the written copy shrinks: the app keeps what it shows, and each agent's own history loads the older requests again
/// when the pane is scrolled up.
pub fn encode_fitting(normalized: &AppSnapshot) -> Result<Vec<u8>, StateError> {
    let encoded = encode(normalized);
    if encoded.len() as u64 <= MAXIMUM_STATE_BYTES {
        return Ok(encoded);
    }
    let mut last = encoded.len();
    for budget in FITTING_GRAPH_BUDGETS {
        let smaller = encode(&trimmed(normalized, budget, None)?);
        if smaller.len() as u64 <= MAXIMUM_STATE_BYTES {
            return Ok(smaller);
        }
        last = smaller.len();
    }
    for keep in FITTING_LOG_ENTRIES {
        let smaller = encode(&trimmed(normalized, 0, Some(keep))?);
        if smaller.len() as u64 <= MAXIMUM_STATE_BYTES {
            return Ok(smaller);
        }
        last = smaller.len();
    }
    Err(StateError::StateTooLarge(last))
}

/// `snapshot` with each pane's diagrams cut to their newest requests within `graph_budget` bytes (the normalizer's own measure) and,
/// given `keep_logs`, its log to that many newest entries.
fn trimmed(snapshot: &AppSnapshot, graph_budget: i64, keep_logs: Option<usize>) -> Result<AppSnapshot, StateError> {
    let mut sessions = Vec::with_capacity(snapshot.sessions().len());
    for session in snapshot.sessions() {
        sessions.push(Arc::new(trimmed_pane(session, graph_budget, keep_logs)?));
    }
    Ok(AppSnapshot { sessions: Some(sessions), ..snapshot.clone() })
}

/// One pane with its diagrams cut to their newest requests within `graph_budget` and, given `keep_logs`, its log to that many entries.
fn trimmed_pane(session: &RunSession, graph_budget: i64, keep_logs: Option<usize>) -> Result<RunSession, StateError> {
    let runs = match session.graph_runs.as_deref() {
        Some(runs) if !runs.is_empty() => {
            let mut budget = graph_budget;
            Some(normalize::graph_runs(runs, false, &mut budget, None).map_err(state_error)?).filter(|r| !r.is_empty())
        }
        _ => session.graph_runs.clone(),
    };
    let logs = match (keep_logs, session.logs.as_deref()) {
        (Some(keep), Some(logs)) if logs.len() > keep => Some(logs[logs.len() - keep..].to_vec()),
        _ => session.logs.clone(),
    };
    Ok(RunSession { graph_runs: runs, logs, ..session.clone() })
}

// ── the split state: an index and one body file per pane ────────────────────────────────────────────────────────────────

/// The folder of the panes' body files, beside the index.
pub const SESSIONS_DIR: &str = "sessions";

/// The largest body file of one pane written or read (as the whole state's was).
pub const MAXIMUM_BODY_BYTES: u64 = MAXIMUM_STATE_BYTES;

/// A pane's body file name: its id when that is a safe file name everywhere (letters, digits, `-`, `_`), else a hash of it; the file
/// names its pane inside (`sessionId`), which the read checks.
pub fn body_file_name(id: &str) -> String {
    if !id.is_empty() && id.len() <= 128 && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_') {
        return format!("{id}.json");
    }
    let mut hash: u64 = 0xcbf29ce484222325;
    for byte in id.as_bytes() {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(0x00000100000001b3);
    }
    format!("h-{hash:016x}.json")
}

/// A pane's incremental append-only journal file name inside `sessions/` (`<stem>.jsonl` where `<stem>.json == body_file_name(id)`).
pub fn journal_file_name(id: &str) -> String {
    let body = body_file_name(id);
    let stem = body.strip_suffix(".json").unwrap_or(&body);
    format!("{stem}.jsonl")
}

pub const JOURNAL_MAX_LINE_BYTES: usize = 256 * 1024;
pub const JOURNAL_MAX_FILE_BYTES: u64 = 512 * 1024;
pub const JOURNAL_MAX_RECORDS: usize = 128;

/// Incremental mutation operation recorded in `sessions/<stem>.jsonl` while a pane is `"running"`.
#[derive(Debug, Clone, PartialEq)]
pub enum BodyJournalOp {
    LogAppend {
        entry: LogEntry,
    },
    LogReplaceLast {
        index: usize,
        entry: LogEntry,
    },
    GraphRunUpsert {
        run: Box<MightyGraphRun>,
    },
    Meta {
        plan_history: Option<Vec<PlanRecord>>,
        todo_progress: Option<TodoProgress>,
        background_work: Option<BackgroundWork>,
        shell_runs: Option<Vec<ShellBlockRun>>,
    },
}

impl Wire for BodyJournalOp {
    fn read(value: &Value) -> json::R<Self> {
        let o = json::Obj::of(value, "a body journal op")?;
        let kind = o.text("op").unwrap_or("");
        match kind {
            "logAppend" => {
                let entry_val = o.get("entry").ok_or_else(|| json::ReadError("missing journal entry".into()))?;
                Ok(BodyJournalOp::LogAppend { entry: LogEntry::read(entry_val)? })
            }
            "logReplaceLast" => {
                let idx = o
                    .get("index")
                    .and_then(json::as_integer)
                    .and_then(|n| usize::try_from(n).ok())
                    .ok_or_else(|| json::ReadError("invalid journal index".into()))?;
                let entry_val = o.get("entry").ok_or_else(|| json::ReadError("missing journal entry".into()))?;
                Ok(BodyJournalOp::LogReplaceLast { index: idx, entry: LogEntry::read(entry_val)? })
            }
            "graphRunUpsert" => {
                let run_val = o.get("run").ok_or_else(|| json::ReadError("missing journal graph run".into()))?;
                Ok(BodyJournalOp::GraphRunUpsert { run: Box::new(MightyGraphRun::read(run_val)?) })
            }
            "meta" => {
                let plan_history = match o.get("planHistory") {
                    Some(Value::Array(items)) => {
                        Some(items.iter().filter(|i| !i.is_null()).filter_map(|i| PlanRecord::read(i).ok()).collect())
                    }
                    _ => None,
                };
                let todo_progress = o.get("todoProgress").filter(|v| !v.is_null()).and_then(|v| TodoProgress::read(v).ok());
                let background_work = o.get("backgroundWork").filter(|v| !v.is_null()).and_then(|v| BackgroundWork::read(v).ok());
                let shell_runs = match o.get("shellRuns") {
                    Some(Value::Array(items)) => {
                        Some(items.iter().filter(|i| !i.is_null()).filter_map(|i| ShellBlockRun::read(i).ok()).collect())
                    }
                    _ => None,
                };
                Ok(BodyJournalOp::Meta { plan_history, todo_progress, background_work, shell_runs })
            }
            other => json::fail(format!("unknown body journal op: {other}")),
        }
    }

    fn write(&self, out: &mut JsonOut) {
        out.begin_object();
        match self {
            BodyJournalOp::LogAppend { entry } => {
                out.prop_str("op", Some("logAppend"));
                out.prop("entry", Some(entry));
            }
            BodyJournalOp::LogReplaceLast { index, entry } => {
                out.prop_str("op", Some("logReplaceLast"));
                out.prop_int("index", *index as i64);
                out.prop("entry", Some(entry));
            }
            BodyJournalOp::GraphRunUpsert { run } => {
                out.prop_str("op", Some("graphRunUpsert"));
                out.name("run");
                run.write_with(out, true);
            }
            BodyJournalOp::Meta { plan_history, todo_progress, background_work, shell_runs } => {
                out.prop_str("op", Some("meta"));
                out.prop_list("planHistory", plan_history.as_ref());
                out.prop("todoProgress", todo_progress.as_ref());
                out.prop("backgroundWork", background_work.as_ref());
                out.prop_list("shellRuns", shell_runs.as_ref());
            }
        }
        out.end_object();
    }
}

/// One append-only journal record in `sessions/<stem>.jsonl`.
#[derive(Debug, Clone, PartialEq)]
pub struct BodyJournalRecord {
    pub rev: u64,
    pub session_id: String,
    pub ops: Vec<BodyJournalOp>,
}

impl Wire for BodyJournalRecord {
    fn read(value: &Value) -> json::R<Self> {
        let o = json::Obj::of(value, "a body journal record")?;
        let rev = o.get("rev").and_then(Value::as_u64).ok_or_else(|| json::ReadError("missing journal rev".into()))?;
        let session_id =
            o.text("sessionId").filter(|s| !s.is_empty()).ok_or_else(|| json::ReadError("missing journal sessionId".into()))?.to_string();
        let ops_arr = o.get("ops").and_then(Value::as_array).ok_or_else(|| json::ReadError("missing journal ops".into()))?;
        let mut ops = Vec::with_capacity(ops_arr.len());
        for item in ops_arr {
            ops.push(BodyJournalOp::read(item)?);
        }
        Ok(BodyJournalRecord { rev, session_id, ops })
    }

    fn write(&self, out: &mut JsonOut) {
        out.begin_object();
        out.prop_int("rev", self.rev as i64);
        out.prop_str("sessionId", Some(&self.session_id));
        out.prop_list("ops", Some(&self.ops));
        out.end_object();
    }
}

impl serde::Serialize for BodyJournalOp {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let value: Value = serde_json::from_str(&self.to_json()).map_err(serde::ser::Error::custom)?;
        value.serialize(serializer)
    }
}

impl<'de> serde::Deserialize<'de> for BodyJournalOp {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = Value::deserialize(deserializer)?;
        BodyJournalOp::read(&value).map_err(serde::de::Error::custom)
    }
}

impl serde::Serialize for BodyJournalRecord {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let value: Value = serde_json::from_str(&self.to_json()).map_err(serde::ser::Error::custom)?;
        value.serialize(serializer)
    }
}

impl<'de> serde::Deserialize<'de> for BodyJournalRecord {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = Value::deserialize(deserializer)?;
        BodyJournalRecord::read(&value).map_err(serde::de::Error::custom)
    }
}

/// Computes purely incremental body operations from `prev` to `current`; returns `None` if front-trimming, reordering, or bulk
/// replacement happened (signalling compaction back into `sessions/<stem>.json`).
pub fn diff_body(prev: &RunSession, current: &RunSession) -> Option<Vec<BodyJournalOp>> {
    let prev_logs = prev.logs();
    let curr_logs = current.logs();
    if curr_logs.len() < prev_logs.len() {
        return None;
    }
    let tail_window_start = prev_logs.len().saturating_sub(8);
    let mut ops = Vec::new();
    let mut replaced_logs = 0usize;
    for i in 0..prev_logs.len() {
        if prev_logs[i].id != curr_logs[i].id {
            return None;
        }
        if prev_logs[i] != curr_logs[i] {
            if i < tail_window_start || replaced_logs >= 4 {
                return None;
            }
            replaced_logs += 1;
            ops.push(BodyJournalOp::LogReplaceLast { index: i, entry: curr_logs[i].clone() });
        }
    }
    for entry in &curr_logs[prev_logs.len()..] {
        ops.push(BodyJournalOp::LogAppend { entry: entry.clone() });
    }

    let prev_runs = prev.graph_runs();
    let curr_runs = current.graph_runs();
    if curr_runs.len() < prev_runs.len() {
        return None;
    }
    let mut upserted_runs = 0usize;
    for i in 0..prev_runs.len() {
        if prev_runs[i].id != curr_runs[i].id {
            return None;
        }
        if prev_runs[i] != curr_runs[i] {
            upserted_runs += 1;
            if upserted_runs > 4 {
                return None;
            }
            ops.push(BodyJournalOp::GraphRunUpsert { run: Box::new(curr_runs[i].clone()) });
        }
    }
    for run in &curr_runs[prev_runs.len()..] {
        upserted_runs += 1;
        if upserted_runs > 4 {
            return None;
        }
        ops.push(BodyJournalOp::GraphRunUpsert { run: Box::new(run.clone()) });
    }

    if prev.plan_history != current.plan_history
        || prev.todo_progress != current.todo_progress
        || prev.background_work != current.background_work
        || prev.shell_block_runs != current.shell_block_runs
    {
        ops.push(BodyJournalOp::Meta {
            plan_history: current.plan_history.clone(),
            todo_progress: current.todo_progress.clone(),
            background_work: current.background_work.clone(),
            shell_runs: current.shell_block_runs.clone(),
        });
    }

    Some(ops)
}

#[derive(Default)]
struct PaneBody {
    body_revision: u64,
    logs: Vec<LogEntry>,
    graph_runs: Option<Vec<MightyGraphRun>>,
    plan_history: Option<Vec<PlanRecord>>,
    todo_progress: Option<TodoProgress>,
    background_work: Option<BackgroundWork>,
    shell_runs: Option<Vec<ShellBlockRun>>,
}

impl PaneBody {
    fn from_map(body: &Map<String, Value>, revision: u64) -> Option<Self> {
        let mut root = Map::new();
        root.insert("id".into(), Value::String("pane".into()));
        for field in crate::model::BODY_FIELDS {
            if let Some(val) = body.get(field) {
                root.insert(field.into(), val.clone());
            }
        }
        let session = RunSession::read(&Value::Object(root)).ok()?;
        Some(PaneBody {
            body_revision: revision,
            logs: session.logs.unwrap_or_default(),
            graph_runs: session.graph_runs,
            plan_history: session.plan_history,
            todo_progress: session.todo_progress,
            background_work: session.background_work,
            shell_runs: session.shell_block_runs,
        })
    }

    fn apply(&mut self, record: BodyJournalRecord) {
        if record.rev <= self.body_revision {
            return;
        }
        for op in record.ops {
            match op {
                BodyJournalOp::LogAppend { entry } => self.logs.push(entry),
                // The entry is found by its id: the writer's `index` is its place in the body the writer had, which the base read here
                // may not be (trimmed at the front, or the `.bak` of an older save). An entry the base does not hold is left out, never
                // laid over another one; `index` stands only for an entry that has no id.
                BodyJournalOp::LogReplaceLast { index, entry } => {
                    if entry.id().is_empty() {
                        if index < self.logs.len() {
                            self.logs[index] = entry;
                        }
                    } else if let Some(at) = self.logs.iter().rposition(|log| log.id() == entry.id()) {
                        self.logs[at] = entry;
                    }
                }
                BodyJournalOp::GraphRunUpsert { run } => {
                    let run = *run;
                    let runs = self.graph_runs.get_or_insert_with(Vec::new);
                    match runs.iter().position(|r| r.id == run.id) {
                        Some(at) => runs[at] = run,
                        None => runs.push(run),
                    }
                }
                BodyJournalOp::Meta { plan_history, todo_progress, background_work, shell_runs } => {
                    self.plan_history = plan_history;
                    self.todo_progress = todo_progress;
                    self.background_work = background_work;
                    self.shell_runs = shell_runs;
                }
            }
        }
        self.body_revision = record.rev;
    }

    fn into_map_and_rev(self) -> Option<(Map<String, Value>, u64)> {
        let rev = self.body_revision;
        let carrier = RunSession {
            logs: Some(self.logs),
            graph_runs: self.graph_runs,
            plan_history: self.plan_history,
            todo_progress: self.todo_progress,
            background_work: self.background_work,
            shell_block_runs: self.shell_runs,
            ..RunSession::default()
        };
        let mut out = JsonOut::new();
        carrier.write_body(&mut out, rev);
        let Value::Object(mut obj) = serde_json::from_str::<Value>(&out.finish()).ok()? else { return None };
        obj.retain(|name, _| crate::model::BODY_FIELDS.contains(&name.as_str()));
        Some((obj, rev))
    }
}

fn read_journal_records(journal_path: &Path, id: &str) -> Vec<BodyJournalRecord> {
    let Ok(meta) = fs::metadata(journal_path) else { return Vec::new() };
    if !meta.is_file() || meta.len() > JOURNAL_MAX_FILE_BYTES * 4 {
        return Vec::new();
    }
    let Ok(bytes) = fs::read(journal_path) else { return Vec::new() };
    let mut records = Vec::new();
    for raw_line in bytes.split(|b| *b == b'\n') {
        let line = raw_line.strip_suffix(b"\r").unwrap_or(raw_line);
        if line.is_empty() {
            continue;
        }
        // Stop cleanly at any torn/corrupted trailing line left by a power cut.
        if damaged(line) {
            break;
        }
        let Ok(text) = std::str::from_utf8(line) else { break };
        let Ok(value) = parse_document(text) else { break };
        let Ok(record) = BodyJournalRecord::read(&value) else { break };
        if record.session_id != id {
            break;
        }
        records.push(record);
    }
    records
}

fn replay_journal_if_present(folder: &Path, id: &str, body: Map<String, Value>, revision: u64) -> (Map<String, Value>, u64) {
    let journal_path = folder.join(journal_file_name(id));
    if !journal_path.exists() {
        return (body, revision);
    }
    let records = read_journal_records(&journal_path, id);
    if !records.iter().any(|r| r.rev > revision) {
        return (body, revision);
    }
    let Some(mut pane_body) = PaneBody::from_map(&body, revision) else {
        return (body, revision);
    };
    for record in records {
        pane_body.apply(record);
    }
    pane_body.into_map_and_rev().unwrap_or((body, revision))
}

/// Reads the full transcript logs of a cold (`body_on_disk`) pane from `<dir>/sessions/<stem>.json` (plus any `.jsonl` replay)
/// without mutating any `Store` state or disk files.
pub fn read_cold_body_logs(dir: &Path, id: &str) -> Option<Vec<LogEntry>> {
    let folder = dir.join(SESSIONS_DIR);
    let name = body_file_name(id);
    let primary = folder.join(&name);
    let backup = folder.join(format!("{name}.bak"));
    let (body, revision) = if primary.exists() {
        read_body_file(&primary, id).ok().or_else(|| backup.exists().then(|| read_body_file(&backup, id).ok()).flatten())?
    } else if backup.exists() {
        read_body_file(&backup, id).ok()?
    } else {
        return None;
    };
    let (replayed, rev) = replay_journal_if_present(&folder, id, body, revision);
    Some(PaneBody::from_map(&replayed, rev)?.logs)
}

/// Reads contiguous incremental `BodyJournalRecord` deltas after `since_rev` for `id` (`MC-13` remote delta sync).
/// Returns `Some(vec![])` when the pane is already at `since_rev`, `Some(records)` when `.jsonl` contiguously covers
/// `(since_rev + 1)..=latest`, or `None` when the requested range was already compacted into `<stem>.json` (or does not exist).
pub fn read_body_deltas_since(dir: &Path, id: &str, since_rev: u64) -> Option<Vec<BodyJournalRecord>> {
    let folder = dir.join(SESSIONS_DIR);
    let name = body_file_name(id);
    let primary = folder.join(&name);
    let (_body, base_rev) = read_body_file(&primary, id).ok()?;
    let journal_path = folder.join(journal_file_name(id));
    let records = read_journal_records(&journal_path, id);
    let latest_rev = records.last().map(|r| r.rev).unwrap_or(base_rev).max(base_rev);
    if since_rev == latest_rev {
        return Some(Vec::new());
    }
    if since_rev > latest_rev {
        return None;
    }
    let after: Vec<BodyJournalRecord> = records.into_iter().filter(|r| r.rev > since_rev).collect();
    if after.is_empty() {
        return None;
    }
    // Ensure contiguous coverage starting immediately at `since_rev + 1` (and check monotonicity).
    for (expected, rec) in (since_rev + 1..).zip(after.iter()) {
        if rec.rev != expected {
            return None;
        }
    }
    Some(after)
}

/// A pane's body file bytes (`{version, sessionId, bodyRevision, ...}`), at most [`MAXIMUM_BODY_BYTES`]: a pane that outgrows it keeps
/// its newest requests (then its newest log entries) in the written copy, as the one state file did.
pub fn encode_body(pane: &RunSession, revision: u64) -> Result<Vec<u8>, StateError> {
    let encode = |pane: &RunSession| {
        let mut out = JsonOut::new();
        pane.write_body(&mut out, revision);
        out.finish().into_bytes()
    };
    let encoded = encode(pane);
    if encoded.len() as u64 <= MAXIMUM_BODY_BYTES {
        return Ok(encoded);
    }
    let mut last = encoded.len();
    for budget in FITTING_GRAPH_BUDGETS {
        let smaller = encode(&trimmed_pane(pane, budget, None)?);
        if smaller.len() as u64 <= MAXIMUM_BODY_BYTES {
            return Ok(smaller);
        }
        last = smaller.len();
    }
    for keep in FITTING_LOG_ENTRIES {
        let smaller = encode(&trimmed_pane(pane, 0, Some(keep))?);
        if smaller.len() as u64 <= MAXIMUM_BODY_BYTES {
            return Ok(smaller);
        }
        last = smaller.len();
    }
    Err(StateError::StateTooLarge(last))
}

/// `copy` (the state written and read back) with the panes whose body `loaded` left on disk marked so again (and each pane's
/// in-memory `body_revision` kept): the mark and the summary are never written, and a pane holding only its summary's entries must
/// never pass for one read whole.
fn with_bodies_on_disk(mut copy: AppSnapshot, loaded: &AppSnapshot) -> AppSnapshot {
    let originals: HashMap<&str, &Arc<RunSession>> = loaded.sessions().iter().map(|s| (s.id(), s)).collect();
    if originals.is_empty() {
        return copy;
    }
    let panes = copy
        .sessions()
        .iter()
        .map(|pane| match originals.get(pane.id()) {
            Some(original) if original.body_on_disk || original.body_revision.is_some() => Arc::new(RunSession {
                body_on_disk: original.body_on_disk,
                stored_digest: if original.body_on_disk { original.stored_digest.clone() } else { None },
                body_revision: original.body_revision,
                ..(**pane).clone()
            }),
            _ => pane.clone(),
        })
        .collect();
    copy.sessions = Some(panes);
    copy
}

/// Whether two states of a pane hold the same body ([`crate::model::BODY_FIELDS`]).
fn same_body(a: &RunSession, b: &RunSession) -> bool {
    a.logs == b.logs
        && a.graph_runs == b.graph_runs
        && a.plan_history == b.plan_history
        && a.todo_progress == b.todo_progress
        && a.background_work == b.background_work
        && a.shell_block_runs == b.shell_block_runs
}

/// The split state's index bytes: the stored state (no files panes) with each pane's body left out and its revision named.
pub fn encode_index(stored: &AppSnapshot, revision: &dyn Fn(&str) -> u64) -> Vec<u8> {
    let mut out = JsonOut::new();
    stored.write_split_index(&mut out, revision);
    out.finish().into_bytes()
}

/// One body file read: its fields (without `version`, `sessionId` and `bodyRevision`) and its revision. Err for a file that is not
/// `id`'s body (damaged, too large, not version 1, another pane's).
fn read_body_file(path: &Path, id: &str) -> Result<(Map<String, Value>, u64), StateError> {
    let size = fs::metadata(path)?.len();
    if size > MAXIMUM_BODY_BYTES {
        return Err(StateError::TooLarge(size));
    }
    let bytes = fs::read(path)?;
    if damaged(&bytes) {
        return Err(StateError::InvalidFormat("damaged".into()));
    }
    let Value::Object(mut body) = parse_document(&text_of(&bytes))? else {
        return Err(StateError::InvalidFormat("not an object".into()));
    };
    if body.remove("version").and_then(|v| v.as_i64()) != Some(1) {
        return Err(StateError::InvalidFormat("version is not 1".into()));
    }
    if body.remove("sessionId").as_ref().and_then(Value::as_str) != Some(id) {
        return Err(StateError::InvalidFormat("another pane's body".into()));
    }
    let revision = body.remove("bodyRevision").and_then(|v| v.as_u64()).unwrap_or(0);
    body.retain(|name, _| crate::model::BODY_FIELDS.contains(&name.as_str()));
    Ok((body, revision))
}

/// A pane's body laid on its index entry. In a split index the body's fields are the pane's. In an index an older app wrote (no
/// marker: the app went back to a version before the split, then forward again) a field the entry holds itself is the newer one, and
/// its requests and log entries go after the body's that the entry does not hold, so nothing written on either side is lost.
pub fn lay_body(entry: &mut Map<String, Value>, body: Map<String, Value>, split: bool) {
    for (name, value) in body {
        let own = entry.get(&name).filter(|v| !v.is_null());
        match own {
            None => {
                entry.insert(name, value);
            }
            Some(_) if split => {
                entry.insert(name, value);
            }
            Some(Value::Array(newer)) if matches!(name.as_str(), "logs" | "graphRuns") => {
                let held: HashSet<&str> = newer.iter().filter_map(|item| item.get("id").and_then(Value::as_str)).collect();
                let Value::Array(older) = value else { continue };
                let mut merged: Vec<Value> =
                    older.into_iter().filter(|item| item.get("id").and_then(Value::as_str).is_none_or(|id| !held.contains(id))).collect();
                merged.extend(newer.iter().cloned());
                entry.insert(name, Value::Array(merged));
            }
            Some(_) => {}
        }
    }
}

/// Removes the body files (and their `.bak` / `.jsonl`) of panes the index no longer lists: a closed pane's data goes as it always
/// did. A file set aside as unreadable stays for the person to look at.
fn remove_unlisted_bodies(folder: &Path, listed: &HashSet<String>) {
    let Ok(entries) = fs::read_dir(folder) else { return };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        let body = name.strip_suffix(".bak").unwrap_or(&name);
        if body.ends_with(".json") && !listed.contains(body) {
            let _ = fs::remove_file(entry.path());
        } else if let Some(stem) = name.strip_suffix(".jsonl")
            && !listed.contains(&format!("{stem}.json"))
        {
            let _ = fs::remove_file(entry.path());
        }
    }
}

/// The profile's private folder for the panes' bodies (owner only on Unix).
fn private_folder(path: &Path) -> io::Result<()> {
    fs::create_dir_all(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

/// Where a load's state came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoadSource {
    /// `workspace-state.json` read as it is.
    State,
    /// The state file was damaged (NUL bytes) and its `.bak` loaded instead.
    Backup,
    /// A default profile without a state file, from the app's folder before the rename.
    Legacy(PathBuf),
    /// No state file: an empty state.
    Empty,
    /// The state file was damaged and there was no readable backup: an empty state.
    DamagedEmpty,
    /// The state file does not read: C# stops loading and every save; this starts empty in memory and says why.
    Unreadable,
}

/// The state of one profile and what may be done to it (C# `StateStore`).
pub struct Store {
    directory: PathBuf,
    legacy: Option<PathBuf>,
    /// The host lets this store write the profile.
    writes: bool,
    loaded: bool,
    approved: IndexMap<String, Workspace>,
    /// The state as the last load or save left it.
    pub snapshot: AppSnapshot,
    /// What the load did with a damaged file, in the user's language, for the window to say once.
    pub load_notice: Option<String>,
    /// The name C# gives a damaged file it moves aside (moved only when writing is on).
    pub aside: Option<String>,
    pub source: Option<LoadSource>,
    /// How the save swaps the new file in (`ReplaceFileW` on Windows; see [`portable_replace`]).
    pub replace: fn(&Path, &Path, &Path) -> io::Result<()>,
    /// The state on disk is split (its index has the marker); until it is, the first save keeps a copy of the one file first.
    split: bool,
    /// Each pane's body revision as last read or written.
    revisions: HashMap<String, u64>,
    /// Monotonic last-written body revision per pane, retained even when `written` drops an evicted pane's `Arc<RunSession>`.
    written_revisions: HashMap<String, u64>,
    /// Count of records currently appended to `sessions/<stem>.jsonl` per pane.
    journal_records: HashMap<String, usize>,
    /// Each pane's normalized state as its body was last written: a save writes a pane's file only when its pane is another object.
    written: HashMap<String, Arc<RunSession>>,
    /// The index bytes last written (an unchanged index is not written again).
    written_index: Option<Vec<u8>>,
    /// The panes a save normalized last, taken again while they are the same objects.
    cache: normalize::PaneCache,
    /// Each pane's summary as the index held it when the state loaded.
    digests: HashMap<String, crate::model::SessionDigest>,
    /// A load of a split state reads the bodies of the active workspace's panes only; the others stay on disk until something asks
    /// for them ([`Store::load_pane_body`]). Off by default (the checks and tools read every body).
    pub lazy: bool,
    /// The panes whose body the load left on disk and no one read since: the store's own record, so a save never takes what such a
    /// pane holds for its whole body, even from a copy of the state that lost the pane's mark.
    on_disk: HashSet<String>,
}

impl Store {
    pub fn new(directory: PathBuf, legacy: Option<PathBuf>, writes: bool) -> Self {
        Store {
            directory,
            legacy,
            writes,
            loaded: false,
            approved: IndexMap::new(),
            snapshot: AppSnapshot::default(),
            load_notice: None,
            aside: None,
            source: None,
            replace: portable_replace,
            split: false,
            revisions: HashMap::new(),
            written_revisions: HashMap::new(),
            journal_records: HashMap::new(),
            written: HashMap::new(),
            written_index: None,
            cache: normalize::PaneCache::default(),
            digests: HashMap::new(),
            lazy: false,
            on_disk: HashSet::new(),
        }
    }

    /// The body of a pane the load left on disk, read now and laid on the pane as the app holds it (its index fields), normalized as
    /// a load normalizes: the pane whole, `body_on_disk` false. A pane whose file is gone or does not read comes back without the
    /// flag and with what it held (its summary's last entries), so it is not asked for again; None for a pane read whole already.
    pub fn load_pane_body(&mut self, pane: &RunSession) -> Option<RunSession> {
        if !pane.body_on_disk && !self.on_disk.contains(pane.id()) {
            return None;
        }
        self.read_whole(pane)
    }

    /// [`Store::load_pane_body`] without its check: the pane's file read and laid on the pane.
    fn read_whole(&mut self, pane: &RunSession) -> Option<RunSession> {
        self.on_disk.remove(pane.id());
        let whole = || RunSession { body_on_disk: false, stored_digest: None, ..pane.clone() };
        let mut notes = vec![];
        let locale = Locale::new("en");
        let Some((body, revision)) = self.read_body(pane.id(), &locale, &mut notes) else { return Some(whole()) };
        let Ok(Value::Object(mut entry)) = serde_json::from_str::<Value>(&pane.to_json()) else { return Some(whole()) };
        for field in crate::model::BODY_FIELDS {
            entry.remove(field);
        }
        lay_body(&mut entry, body, true);
        let remote = self.snapshot.workspace(pane.workspace_id()).is_some_and(|w| w.remote.is_some());
        let ssh = self.snapshot.workspace(pane.workspace_id()).is_some_and(|w| w.ssh.is_some());
        let read = RunSession::read(&Value::Object(entry)).ok().and_then(|read| normalize::restored_pane_in(&read, remote, ssh).ok());
        let Some(read) = read else { return Some(whole()) };
        let known = self.body_revision(pane.id()).max(revision);
        if known > 0 {
            self.revisions.insert(pane.id().to_string(), known);
            self.written_revisions.insert(pane.id().to_string(), known);
        }
        let loaded = RunSession { body_on_disk: false, stored_digest: None, body_revision: (known > 0).then_some(known), ..read };
        // What the file holds now: the next save writes the pane again only when its body changes from this (only when the primary
        // file exists and no uncompacted `.jsonl` sits beside it; otherwise leave `written` unset so the next save compacts/recreates).
        let sessions = self.sessions_dir();
        if sessions.join(body_file_name(pane.id())).exists() && !sessions.join(journal_file_name(pane.id())).exists() {
            self.written.insert(pane.id().to_string(), Arc::new(loaded.clone()));
        }
        if let Some(at) = self.snapshot.sessions().iter().position(|s| s.id() == pane.id()) {
            self.snapshot.sessions_mut()[at] = Arc::new(loaded.clone());
        }
        Some(loaded)
    }

    /// Compacts `sessions/<stem>.jsonl` into the canonical `sessions/<stem>.json` and deletes the `.jsonl` file.
    /// Returns `true` if a journal existed and was compacted.
    pub fn compact_pane_journal(&mut self, id: &str) -> bool {
        if !self.writes || !self.loaded {
            return false;
        }
        let folder = self.sessions_dir();
        let journal_path = folder.join(journal_file_name(id));
        if !journal_path.exists() {
            self.journal_records.remove(id);
            return false;
        }
        let body_name = body_file_name(id);
        let body_path = folder.join(&body_name);
        let backup_path = folder.join(format!("{body_name}.bak"));

        let hot_pane = self.snapshot.session(id).filter(|s| !s.body_on_disk && !self.on_disk.contains(id)).cloned();
        let (target_pane, rev) = if let Some(pane) = hot_pane {
            let same = self.written.get(id).is_some_and(|w| Arc::ptr_eq(w, &pane) || same_body(w, &pane));
            let base = self.body_revision(id).max(pane.body_revision.unwrap_or(0));
            let rev = if same && base > 0 { base } else { base + 1 };
            ((*pane).clone(), rev)
        } else {
            let mut notes = Vec::new();
            let Some((body_map, rev)) = self.read_body(id, &Locale::new("en"), &mut notes) else {
                return false;
            };
            let Some(pane_body) = PaneBody::from_map(&body_map, rev) else { return false };
            let carrier = RunSession {
                id: Some(id.to_string()),
                logs: Some(pane_body.logs),
                graph_runs: pane_body.graph_runs,
                plan_history: pane_body.plan_history,
                todo_progress: pane_body.todo_progress,
                background_work: pane_body.background_work,
                shell_block_runs: pane_body.shell_runs,
                ..RunSession::default()
            };
            (carrier, rev.max(1))
        };

        if private_folder(&folder).is_err() {
            return false;
        }
        let Ok(bytes) = encode_body(&target_pane, rev) else { return false };
        if atomic_write(&body_path, &bytes, Some(&backup_path), self.replace).is_err() {
            return false;
        }
        let _ = fs::remove_file(&journal_path);
        self.journal_records.remove(id);
        self.revisions.insert(id.to_string(), rev);
        self.written_revisions.insert(id.to_string(), rev);
        if let Some(at) = self.snapshot.sessions().iter().position(|s| s.id() == id) {
            let existing = &self.snapshot.sessions()[at];
            if !existing.body_on_disk && !self.on_disk.contains(id) {
                let updated = Arc::new(RunSession { body_revision: Some(rev), ..target_pane });
                self.written.insert(id.to_string(), updated.clone());
                self.snapshot.sessions_mut()[at] = updated;
            }
        }
        true
    }

    /// Evicts a loaded pane's full body from memory, leaving only `SessionDigest`'s tail in `self.snapshot` (`body_on_disk: true`).
    /// - When `!self.writes`, evicts only if `<stem>.json` already exists on disk without an uncompacted `.jsonl` or unsaved diff.
    /// - When `self.writes`, flushes any unsaved body diff and compacts any `.jsonl` into `<stem>.json` before dropping the hot body.
    pub fn evict_loaded_body(&mut self, id: &str) -> Option<RunSession> {
        let pane = self.snapshot.sessions().iter().find(|s| s.id() == id)?.clone();
        self.evict_live_body(&pane)
    }

    /// [`Store::evict_loaded_body`] for the pane as its owner (the run service) holds it now, which may be ahead of this store's last
    /// saved copy: that exact pane's body is what gets flushed and what the cold copy is built from, so nothing unsaved (log entries,
    /// status, title, draft, usage) is lost to the eviction. Without writes it evicts only when the file already holds that body.
    pub fn evict_live_body(&mut self, live: &Arc<RunSession>) -> Option<RunSession> {
        let id = live.id();
        let at = self.snapshot.sessions().iter().position(|s| s.id() == id)?;
        let pane = live.clone();
        if pane.body_on_disk || self.snapshot.sessions()[at].body_on_disk || self.on_disk.contains(id) {
            return None;
        }
        if !matches!(pane.kind(), "claude" | "shell") || pane.mirror.is_some() {
            return None;
        }
        let folder = self.sessions_dir();
        let body_name = body_file_name(id);
        let body_path = folder.join(&body_name);
        let journal_path = folder.join(journal_file_name(id));

        if !self.writes {
            if !body_path.exists() || journal_path.exists() {
                return None;
            }
            if let Some(last) = self.written.get(id) {
                if !Arc::ptr_eq(last, &pane) && !same_body(last, &pane) {
                    return None;
                }
            } else {
                let (disk_map, disk_rev) = read_body_file(&body_path, id).ok()?;
                let disk_body = PaneBody::from_map(&disk_map, disk_rev)?;
                let disk_session = RunSession {
                    logs: Some(disk_body.logs),
                    graph_runs: disk_body.graph_runs,
                    plan_history: disk_body.plan_history,
                    todo_progress: disk_body.todo_progress,
                    background_work: disk_body.background_work,
                    shell_block_runs: disk_body.shell_runs,
                    ..(*pane).clone()
                };
                let remote = self.snapshot.workspace(pane.workspace_id()).is_some_and(|w| w.remote.is_some());
                let ssh = self.snapshot.workspace(pane.workspace_id()).is_some_and(|w| w.ssh.is_some());
                let normalized_disk = normalize::restored_pane_in(&disk_session, remote, ssh).unwrap_or(disk_session);
                if !same_body(&normalized_disk, &pane) {
                    return None;
                }
            }
        } else {
            let needs_flush = !body_path.exists()
                || journal_path.exists()
                || self.written.get(id).is_none_or(|last| !Arc::ptr_eq(last, &pane) && !same_body(last, &pane));
            if needs_flush {
                private_folder(&folder).ok()?;
                let same_as_written =
                    body_path.exists() && self.written.get(id).is_some_and(|last| Arc::ptr_eq(last, &pane) || same_body(last, &pane));
                let base = self.body_revision(id).max(pane.body_revision.unwrap_or(0));
                let rev = if same_as_written && base > 0 { base } else { base + 1 };
                let bytes = encode_body(&pane, rev).ok()?;
                let backup_path = folder.join(format!("{body_name}.bak"));
                atomic_write(&body_path, &bytes, Some(&backup_path), self.replace).ok()?;
                if journal_path.exists() {
                    let _ = fs::remove_file(&journal_path);
                }
                self.journal_records.remove(id);
                self.revisions.insert(id.to_string(), rev);
                self.written_revisions.insert(id.to_string(), rev);
            } else {
                let rev = self.body_revision(id).max(pane.body_revision.unwrap_or(0));
                if rev > 0 {
                    self.revisions.insert(id.to_string(), rev);
                    self.written_revisions.insert(id.to_string(), rev);
                }
            }
        }

        let rev = self.body_revision(id).max(pane.body_revision.unwrap_or(0));
        let mut with_rev = (*pane).clone();
        if rev > 0 {
            with_rev.body_revision = Some(rev);
        }
        let cold = with_rev.evicted_to_disk();
        self.digests.insert(id.to_string(), cold.body_digest());
        self.snapshot.sessions_mut()[at] = Arc::new(cold.clone());
        self.written.remove(id);
        self.cache.remove(id);
        self.on_disk.insert(id.to_string());
        Some(cold)
    }

    /// A pane the load left on disk, as a save writes it: unchanged (only its summary's entries, no other body field), nothing to
    /// write; changed (something added to it before its body was read), its file's body with the additions laid over by id.
    fn body_with_additions(&mut self, pane: &RunSession) -> Option<RunSession> {
        let digest = pane.stored_digest.as_deref().or_else(|| self.digests.get(pane.id()));
        let known: HashSet<String> = digest.map(|d| d.tail.iter().map(|e| e.id().to_string()).collect()).unwrap_or_default();
        let added_logs: Vec<&crate::model::LogEntry> = pane.logs().iter().filter(|e| !known.contains(e.id())).collect();
        let untouched = added_logs.is_empty()
            && pane.graph_runs().is_empty()
            && pane.plan_history.is_none()
            && pane.todo_progress.is_none()
            && pane.background_work.is_none()
            && pane.shell_block_runs.is_none();
        if untouched {
            return None;
        }
        let mut whole = self.read_whole(pane)?;
        // Written now: what it holds stays the app's (still marked on disk there), and its file is the whole body from here on.
        self.on_disk.insert(pane.id().to_string());
        let mut logs = whole.logs().to_vec();
        for entry in added_logs {
            if !logs.iter().any(|e| e.id == entry.id) {
                logs.push(entry.clone());
            }
        }
        whole.logs = Some(logs);
        let mut runs = whole.graph_runs().to_vec();
        for run in pane.graph_runs() {
            match runs.iter().position(|r| r.id == run.id) {
                Some(at) => runs[at] = run.clone(),
                None => runs.push(run.clone()),
            }
        }
        whole.graph_runs = Some(runs);
        whole.plan_history = pane.plan_history.clone().or(whole.plan_history);
        whole.todo_progress = pane.todo_progress.clone().or(whole.todo_progress);
        whole.background_work = pane.background_work.clone().or(whole.background_work);
        whole.shell_block_runs = pane.shell_block_runs.clone().or(whole.shell_block_runs);
        Some(whole)
    }

    /// A pane's body revision as last read or written (0 for none): what a remote reader can ask "changed since" with.
    pub fn body_revision(&self, id: &str) -> u64 {
        self.revisions.get(id).copied().or_else(|| self.written_revisions.get(id).copied()).unwrap_or(0)
    }

    /// A pane's summary as the index held it when the state loaded.
    pub fn index_digest(&self, id: &str) -> Option<&crate::model::SessionDigest> {
        self.digests.get(id)
    }

    /// The folder of the panes' body files.
    pub fn sessions_dir(&self) -> PathBuf {
        self.directory.join(SESSIONS_DIR)
    }

    /// Whether the state on disk is the split one (its index named so when it loaded, or written so since).
    pub fn is_split(&self) -> bool {
        self.split
    }

    /// The state file's bytes as a load reads them: the index (or the one file of an older state) checked as [`read_state`] checks
    /// it, with each pane's body file laid on its entry first. A body file that does not read is moved aside (with writing on) and
    /// its `.bak` taken, else the pane opens without its body; a note in `locale` says which file was left.
    fn read_split(&mut self, bytes: &[u8], locale: &Locale) -> Result<AppSnapshot, StateError> {
        if bytes.len() as u64 > MAXIMUM_STATE_BYTES {
            return Err(StateError::TooLarge(bytes.len() as u64));
        }
        let mut value = parse_document(&text_of(bytes))?;
        self.split = false;
        self.revisions.clear();
        self.written_revisions.clear();
        self.journal_records.clear();
        self.digests.clear();
        self.on_disk.clear();
        self.written.clear();
        self.written_index = None;
        let mut notes = vec![];
        let mut on_disk: HashSet<String> = HashSet::new();
        if let Value::Object(root) = &mut value {
            self.split = root.remove(crate::model::SPLIT_MARKER).and_then(|v| v.as_i64()) == Some(1);
            let active = root.get("activeWorkspaceId").and_then(Value::as_str).map(str::to_string);
            let lazy = self.lazy && self.split;
            if let Some(Value::Array(panes)) = root.get_mut("sessions") {
                for pane in panes.iter_mut() {
                    let Some(entry) = pane.as_object_mut() else { continue };
                    let revision = entry.remove("bodyRevision").and_then(|v| v.as_u64());
                    // The index's summary of the body, kept for a pane whose body is read later; the body read below is the pane's own.
                    if let Some(digest) = entry.remove("digest").as_ref().and_then(crate::model::SessionDigest::read)
                        && let Some(id) = entry.get("id").and_then(Value::as_str)
                    {
                        self.digests.insert(id.to_string(), digest);
                    }
                    let Some(id) = entry.get("id").and_then(Value::as_str).map(str::to_string) else { continue };
                    if let Some(revision) = revision {
                        self.revisions.insert(id.clone(), revision);
                        self.written_revisions.insert(id.clone(), revision);
                    }
                    // Outside the active workspace, a pane with a summary and a compacted body file keeps its body on disk: what it
                    // holds is the summary's entries until something reads it. If an uncompacted `.jsonl` exists from a crash, read
                    // and replay it now so its recovery and revision are up to date.
                    let elsewhere = entry.get("workspaceId").and_then(Value::as_str) != active.as_deref();
                    let sessions = self.sessions_dir();
                    if lazy
                        && elsewhere
                        && let Some(digest) = self.digests.get(&id)
                        && sessions.join(body_file_name(&id)).exists()
                        && !sessions.join(journal_file_name(&id)).exists()
                    {
                        let tail: Vec<Value> =
                            digest.tail.iter().filter_map(|e| serde_json::from_str(&crate::json::Wire::to_json(e)).ok()).collect();
                        entry.insert("logs".into(), Value::Array(tail));
                        on_disk.insert(id.clone());
                        self.on_disk.insert(id.clone());
                        continue;
                    }
                    if let Some((body, revision)) = self.read_body(&id, locale, &mut notes) {
                        let known = self.revisions.get(&id).copied().unwrap_or(0).max(revision);
                        self.revisions.insert(id.clone(), known);
                        self.written_revisions.insert(id.clone(), known);
                        lay_body(entry, body, self.split);
                    }
                }
            }
        }
        if !notes.is_empty() {
            let mut all = self.load_notice.take().into_iter().collect::<Vec<_>>();
            all.extend(notes);
            self.load_notice = Some(all.join("\n"));
        }
        let mut snapshot = read_state_value(value, locale)?;
        if !on_disk.is_empty() || !self.revisions.is_empty() {
            let panes = snapshot
                .sessions()
                .iter()
                .map(|pane| {
                    let rev = self.revisions.get(pane.id()).copied().filter(|r| *r > 0);
                    match self.digests.get(pane.id()).filter(|_| on_disk.contains(pane.id())) {
                        Some(digest) => Arc::new(RunSession {
                            body_on_disk: true,
                            stored_digest: Some(Arc::new(digest.clone())),
                            body_revision: rev,
                            ..(**pane).clone()
                        }),
                        None if rev.is_some() => Arc::new(RunSession { body_revision: rev, ..(**pane).clone() }),
                        None => pane.clone(),
                    }
                })
                .collect();
            snapshot.sessions = Some(panes);
        }
        Ok(snapshot)
    }

    /// `id`'s body file, or its `.bak` when the primary file is missing or does not read (a corrupt primary is moved aside first,
    /// with writing on), with any `sessions/<stem>.jsonl` records newer than the base `bodyRevision` replayed onto it; none when
    /// neither exists or decodes.
    fn read_body(&self, id: &str, locale: &Locale, notes: &mut Vec<String>) -> Option<(Map<String, Value>, u64)> {
        let folder = self.sessions_dir();
        let name = body_file_name(id);
        let path = folder.join(&name);
        let backup = folder.join(format!("{name}.bak"));
        let base = if !path.exists() {
            backup.exists().then(|| read_body_file(&backup, id).ok()).flatten()
        } else {
            match read_body_file(&path, id) {
                Ok(body) => Some(body),
                Err(_) => {
                    let aside = format!("{name}.unreadable-{}-{}", chrono::Local::now().format("%Y%m%d-%H%M%S"), &new_id()[..8]);
                    if self.writes {
                        let _ = fs::rename(&path, folder.join(&aside));
                    }
                    let restored = read_body_file(&backup, id).ok();
                    let key = if restored.is_some() { "store.notice.paneRestoredBackup" } else { "store.notice.paneStartedEmpty" };
                    notes.push(locale.get_with(key, &[("file", &format!("{SESSIONS_DIR}/{aside}"))]));
                    restored
                }
            }
        }?;
        Some(replay_journal_if_present(&folder, id, base.0, base.1))
    }

    /// Reads `workspace-state.json.bak` if it exists and is valid; restores store metadata on corrupt/unreadable backup.
    fn read_backup(&mut self, locale: &Locale) -> Result<Option<AppSnapshot>, StateError> {
        let backup = self.backup_path();
        if !backup.exists() {
            return Ok(None);
        }
        let bytes = fs::read(&backup).map_err(StateError::Io)?;
        if damaged(&bytes) {
            return Ok(None);
        }
        let previous_notice = self.load_notice.clone();
        match self.read_split(&bytes, locale) {
            Ok(snapshot) => Ok(Some(snapshot)),
            Err(StateError::Io(e)) => Err(StateError::Io(e)),
            Err(StateError::Json(_) | StateError::InvalidFormat(_) | StateError::TooLarge(_)) => {
                self.split = false;
                self.revisions.clear();
                self.written_revisions.clear();
                self.journal_records.clear();
                self.digests.clear();
                self.on_disk.clear();
                self.written.clear();
                self.written_index = None;
                self.load_notice = previous_notice;
                Ok(None)
            }
            Err(other) => Err(other),
        }
    }

    pub fn directory(&self) -> &Path {
        &self.directory
    }

    pub fn state_path(&self) -> PathBuf {
        self.directory.join(STATE_FILE)
    }

    fn backup_path(&self) -> PathBuf {
        self.directory.join(format!("{STATE_FILE}.bak"))
    }

    pub fn writes(&self) -> bool {
        self.writes
    }

    pub fn is_loaded(&self) -> bool {
        self.loaded
    }

    /// C# `LoadAsync`. On failure the store stays unloaded and refuses every save. `locale` is the language the notes are written
    /// in (C# loads before the saved language preference is applied: the OS's).
    pub fn load(&mut self, locale: &Locale) -> Result<AppSnapshot, StateError> {
        if self.writes {
            private_folder(&self.directory)?;
        }
        let state = self.state_path();
        let mut path = state.clone();
        self.source = Some(LoadSource::Empty);
        let mut recovered_from_backup = false;
        if !state.exists()
            && let Some(snapshot) = self.read_backup(locale)?
        {
            self.source = Some(LoadSource::Backup);
            self.snapshot = snapshot;
            recovered_from_backup = true;
        }
        if !recovered_from_backup
            && !state.exists()
            && let Some(legacy) = &self.legacy
        {
            let original = legacy.join(STATE_FILE);
            if original.exists() {
                let size = fs::metadata(&original)?.len();
                if size > MAXIMUM_STATE_BYTES {
                    return Err(StateError::LegacyTooLarge(size));
                }
                if self.writes {
                    copy_new(&original, &state)?;
                } else {
                    path = original.clone();
                }
                self.source = Some(LoadSource::Legacy(original));
            }
        }
        if recovered_from_backup {
            // Already recovered into self.snapshot from `.bak`.
        } else if path.exists() {
            let bytes = fs::read(&path)?;
            if damaged(&bytes) {
                let aside = format!("{STATE_FILE}.unreadable-{}-{}", chrono::Local::now().format("%Y%m%d-%H%M%S"), &new_id()[..8]);
                if self.writes {
                    fs::rename(&state, self.directory.join(&aside))?;
                }
                let restored = self.read_backup(locale)?;
                let key = if restored.is_some() { "store.notice.restoredBackup" } else { "store.notice.unreadableStartedEmpty" };
                self.source = Some(if restored.is_some() { LoadSource::Backup } else { LoadSource::DamagedEmpty });
                self.snapshot = restored.unwrap_or_else(|| AppSnapshot { antigravity_migrated: Some(true), ..AppSnapshot::default() });
                self.load_notice = Some(locale.get_with(key, &[("file", &aside)]));
                self.aside = Some(aside);
            } else {
                match self.read_split(&bytes, locale) {
                    Ok(snapshot) => {
                        self.snapshot = snapshot;
                        if !matches!(self.source, Some(LoadSource::Legacy(_))) {
                            self.source = Some(LoadSource::State);
                        }
                    }
                    Err(error) => {
                        self.source = Some(LoadSource::Unreadable);
                        return Err(error);
                    }
                }
            }
        } else {
            self.snapshot = AppSnapshot { antigravity_migrated: Some(true), ..AppSnapshot::default() };
        }
        self.approved = self.snapshot.workspaces().iter().map(|w| (w.id().to_string(), w.clone())).collect();
        self.loaded = true;
        Ok(with_bodies_on_disk(clone_state(&self.snapshot), &self.snapshot))
    }

    /// C# `ApproveLocal`: a folder the user picked, as a workspace (the one listed already for it, if any).
    pub fn approve_local(&mut self, path: &Path) -> io::Result<Workspace> {
        let full = std::path::absolute(path)?;
        if !full.is_dir() {
            return Err(io::Error::new(io::ErrorKind::NotFound, "store.error.projectFolderNotFound"));
        }
        let text = full.to_string_lossy().to_string();
        if let Some(existing) = self.approved.values().find(|w| w.remote.is_none() && w.path() == text) {
            return Ok(existing.clone());
        }
        let trimmed = text.trim_end_matches(['\\', '/']);
        let name = Path::new(trimmed).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        let workspace = Workspace { name: Some(name), path: Some(text), ..Workspace::default() };
        self.approved.insert(workspace.id().to_string(), workspace.clone());
        Ok(workspace)
    }

    /// Whether a save may write `workspace` as it is: the load or the user approved it with this path (and remote reference).
    pub fn approves(&self, workspace: &Workspace) -> bool {
        self.approved
            .get(workspace.id())
            .is_some_and(|original| original.path == workspace.path && original.remote == workspace.remote && original.ssh == workspace.ssh)
    }

    /// MC-33: a folder on an SSH machine, as a workspace a save may write (the one listed for that machine and folder, if any). `local`
    /// is the app's empty stand-in folder for it (made by the caller). None when a name is not one the store keeps.
    pub fn approve_ssh(&mut self, machine: &str, host: &str, dir: &str, local: impl FnOnce(&str) -> String) -> Option<Workspace> {
        let host = wire::clean(host, 120).trim().to_string();
        if !wire::identifier(machine) || !crate::ssh::valid_remote_dir(dir) || host.is_empty() {
            return None;
        }
        if let Some(existing) =
            self.approved.values().find(|w| w.ssh.as_ref().is_some_and(|r| r.machine == machine && r.dir == dir)).cloned()
        {
            return Some(existing);
        }
        let id = new_id();
        let path = local(&id);
        if !normalize::fully_qualified(&path) {
            return None;
        }
        let item = Workspace {
            id: Some(id),
            name: Some(wire::clean(&crate::ssh::dir_name(dir, &host), 120)),
            path: Some(path),
            ssh: Some(crate::model::SshWorkspaceRef { machine: machine.into(), dir: dir.into(), host }),
            ..Workspace::default()
        };
        self.approved.insert(item.id().to_string(), item.clone());
        Some(item)
    }

    /// C# `ApproveRemote`: a workspace another computer shares.
    pub fn approve_remote(
        &mut self,
        connection_id: &str,
        host_workspace_id: &str,
        name: &str,
        path: &str,
        host_name: &str,
    ) -> Option<Workspace> {
        let host = wire::clean(host_name, 120).trim().to_string();
        if !wire::identifier(connection_id)
            || !wire::identifier(host_workspace_id)
            || !normalize::remote_path(Some(path))
            || host.is_empty()
        {
            return None;
        }
        let existing = self
            .approved
            .values()
            .find(|w| {
                w.remote.as_ref().is_some_and(|r| {
                    r.connection_id.as_deref() == Some(connection_id) && r.workspace_id.as_deref() == Some(host_workspace_id)
                })
            })
            .cloned();
        let item = Workspace {
            id: Some(existing.as_ref().map(|w| w.id().to_string()).unwrap_or_else(new_id)),
            name: Some(wire::clean(name, 120)),
            path: Some(path.into()),
            created_at: existing.and_then(|w| w.created_at).or_else(|| Some(crate::time::now_o())),
            model_defaults: None,
            remote: Some(RemoteWorkspaceReference {
                connection_id: Some(connection_id.into()),
                workspace_id: Some(host_workspace_id.into()),
                host_name: Some(host),
                version: 2,
                mirror: false,
            }),
            ..Workspace::default()
        };
        self.approved.insert(item.id().to_string(), item.clone());
        Some(item)
    }

    /// C# `SaveAsync`: normalized, checked, written, as the split state: the body file (or incremental `.jsonl` line while
    /// `"running"`) of each pane that changed since its last write, then the index, then the body/journal files of panes no longer
    /// listed removed. A crash between leaves bodies/journals newer than the index, which loads. The normalized state is kept as the
    /// store's.
    pub fn save(&mut self, snapshot: &AppSnapshot) -> Result<(), StateError> {
        enum BodyPlan {
            Canonical(Vec<u8>),
            JournalAppend(Vec<u8>),
        }

        if !self.loaded {
            return Err(StateError::NotLoaded);
        }
        // The remote mirror's workspaces and panes are another computer's, held while connected: never written.
        let mut normalized = normalize::normalize_with(&snapshot.without_mirrors(), false, Some(&mut self.cache)).map_err(state_error)?;
        for workspace in normalized.workspaces() {
            if !self.approves(workspace) {
                return Err(StateError::PathNotApproved);
            }
        }
        let stored = normalize::stored(&normalized);
        let folder = self.sessions_dir();
        let mut bodies: Vec<(Arc<RunSession>, u64, BodyPlan)> = vec![];
        // A pane whose body was never read: its index entry keeps the summary it had, and its file is written only when something was
        // added to it meanwhile (laid over the file's body, never in place of it).
        let mut whole_of: HashMap<String, RunSession> = HashMap::new();
        let held: Vec<Arc<RunSession>> =
            stored.sessions().iter().filter(|p| p.body_on_disk || self.on_disk.contains(p.id())).cloned().collect();
        for pane in &held {
            if self.written.get(pane.id()).is_some_and(|last| Arc::ptr_eq(last, pane) || same_body(last, pane)) {
                continue;
            }
            match self.body_with_additions(pane) {
                Some(whole) => {
                    whole_of.insert(pane.id().to_string(), whole);
                }
                None => {
                    self.written.insert(pane.id().to_string(), pane.clone());
                }
            }
        }
        let mut merged: HashMap<String, crate::model::SessionDigest> = HashMap::new();
        for pane in stored.sessions() {
            let id = pane.id();
            if pane.body_on_disk || self.on_disk.contains(id) {
                let Some(whole) = whole_of.remove(id) else { continue };
                merged.insert(id.to_string(), crate::model::SessionDigest::of(&whole));
                let revision = self.body_revision(id).max(pane.body_revision.unwrap_or(0)) + 1;
                let bytes = encode_body(&whole, revision)?;
                // Kept as the pane the app holds, so the next save finds it written.
                bodies.push((pane.clone(), revision, BodyPlan::Canonical(bytes)));
                continue;
            }

            let journal_path = folder.join(journal_file_name(id));
            let journal_exists = journal_path.exists() || self.journal_records.get(id).copied().unwrap_or(0) > 0;
            // Every change the window makes normalizes the whole state, so an untouched pane is often another object with the same
            // body: compared by its body's fields then, and written only when they differ (or compacted when a run just ended with an
            // open `.jsonl`).
            if self.written.get(id).is_some_and(|last| Arc::ptr_eq(last, pane) || same_body(last, pane)) {
                if pane.status() != "running" && journal_exists {
                    let revision = self.body_revision(id).max(pane.body_revision.unwrap_or(0)).max(1);
                    bodies.push((pane.clone(), revision, BodyPlan::Canonical(encode_body(pane, revision)?)));
                } else {
                    self.written.insert(id.to_string(), pane.clone());
                }
                continue;
            }

            let revision = self.body_revision(id).max(pane.body_revision.unwrap_or(0)) + 1;
            let body_path = folder.join(body_file_name(id));
            let append_line = if pane.status() == "running"
                && body_path.exists()
                && let Some(prev) = self.written.get(id)
                && let Some(ops) = diff_body(prev, pane).filter(|ops| !ops.is_empty())
            {
                let record = BodyJournalRecord { rev: revision, session_id: id.to_string(), ops };
                let mut line = record.to_json().into_bytes();
                line.push(b'\n');
                let existing_meta = fs::metadata(&journal_path).ok();
                let existing_bytes = existing_meta.as_ref().map_or(0, fs::Metadata::len);
                let existing_lines = match self.journal_records.get(id).copied() {
                    Some(count) => count,
                    None if existing_bytes > 0 => read_journal_records(&journal_path, id).len(),
                    None => 0,
                };
                (line.len() < JOURNAL_MAX_LINE_BYTES
                    && existing_bytes < JOURNAL_MAX_FILE_BYTES
                    && existing_bytes + line.len() as u64 <= JOURNAL_MAX_FILE_BYTES
                    && existing_lines < JOURNAL_MAX_RECORDS)
                    .then_some(line)
            } else {
                None
            };

            match append_line {
                Some(line) => bodies.push((pane.clone(), revision, BodyPlan::JournalAppend(line))),
                None => bodies.push((pane.clone(), revision, BodyPlan::Canonical(encode_body(pane, revision)?))),
            }
        }
        // A pane whose body was laid over its file names its new summary in the index.
        let mut stored = stored;
        if !merged.is_empty() {
            let panes = stored
                .sessions()
                .iter()
                .map(|pane| match merged.get(pane.id()) {
                    Some(digest) => Arc::new(RunSession { stored_digest: Some(Arc::new(digest.clone())), ..(**pane).clone() }),
                    None => pane.clone(),
                })
                .collect();
            stored.sessions = Some(panes);
        }
        let fresh: HashMap<&str, u64> = bodies.iter().map(|(pane, revision, _)| (pane.id(), *revision)).collect();
        let index = encode_index(&stored, &|id| fresh.get(id).copied().unwrap_or_else(|| self.body_revision(id)));
        if index.len() as u64 > MAXIMUM_STATE_BYTES {
            return Err(StateError::StateTooLarge(index.len()));
        }
        if !self.writes {
            return Err(StateError::WritesOff);
        }
        private_folder(&self.directory)?;
        let state = self.state_path();
        if !self.split && state.exists() {
            // The one file of a state before the split, kept once as it was (owner-only on Unix).
            let kept = self.directory.join(format!("{STATE_FILE}.before-session-files-{}", chrono::Local::now().format("%Y%m%d-%H%M%S")));
            if !kept.exists() {
                match copy_new(&state, &kept) {
                    Ok(()) => {}
                    Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
                    Err(error) => return Err(error.into()),
                }
            }
        }
        if !bodies.is_empty() || !folder.exists() {
            private_folder(&folder)?;
        }
        for (pane, revision, plan) in bodies {
            let id = pane.id().to_string();
            let name = body_file_name(&id);
            let journal_path = folder.join(journal_file_name(&id));
            match plan {
                BodyPlan::JournalAppend(line) => {
                    append_journal_line(&journal_path, &line)?;
                    let count = self.journal_records.entry(id.clone()).or_insert(0);
                    *count += 1;
                }
                BodyPlan::Canonical(bytes) => {
                    atomic_write(&folder.join(&name), &bytes, Some(&folder.join(format!("{name}.bak"))), self.replace)?;
                    if journal_path.exists() {
                        let _ = fs::remove_file(&journal_path);
                    }
                    self.journal_records.remove(&id);
                }
            }
            self.revisions.insert(id.clone(), revision);
            self.written_revisions.insert(id.clone(), revision);
            let with_rev = if pane.body_revision == Some(revision) {
                pane
            } else {
                Arc::new(RunSession { body_revision: Some(revision), ..(*pane).clone() })
            };
            self.written.insert(id, with_rev);
        }
        if self.written_index.as_deref() != Some(index.as_slice()) || !state.exists() {
            atomic_write(&state, &index, Some(&self.backup_path()), self.replace)?;
            self.written_index = Some(index);
        }
        self.split = true;
        let listed: HashSet<String> = stored.sessions().iter().map(|s| body_file_name(s.id())).collect();
        self.written.retain(|id, _| listed.contains(&body_file_name(id)));
        self.revisions.retain(|id, _| listed.contains(&body_file_name(id)));
        self.written_revisions.retain(|id, _| listed.contains(&body_file_name(id)));
        self.journal_records.retain(|id, _| listed.contains(&body_file_name(id)));
        self.on_disk.retain(|id| listed.contains(&body_file_name(id)));
        remove_unlisted_bodies(&folder, &listed);
        let stamped: Vec<Arc<RunSession>> = normalized
            .sessions()
            .iter()
            .map(|pane| {
                let rev = self.body_revision(pane.id());
                if rev > 0 && pane.body_revision != Some(rev) {
                    Arc::new(RunSession { body_revision: Some(rev), ..(**pane).clone() })
                } else {
                    pane.clone()
                }
            })
            .collect();
        normalized.sessions = Some(stamped);
        self.snapshot = normalized;
        Ok(())
    }
}

/// Appends one compact JSON line (`O_APPEND`, `0o600` on Unix, `sync_data()`) to `sessions/<stem>.jsonl`.
fn append_journal_line(path: &Path, line: &[u8]) -> io::Result<()> {
    #[cfg(unix)]
    let created = !path.exists();
    let mut options = fs::OpenOptions::new();
    options.create(true).append(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    let mut file = options.open(path)?;
    #[cfg(unix)]
    if created {
        use std::os::unix::fs::PermissionsExt;
        let _ = file.set_permissions(fs::Permissions::from_mode(0o600));
    }
    file.write_all(line)?;
    file.sync_data()?;
    drop(file);
    #[cfg(unix)]
    if created {
        sync_parent(path);
    }
    Ok(())
}

/// `File.Copy(from, to, overwrite: false)`, owner-only on Unix and synced to disk.
fn copy_new(from: &Path, to: &Path) -> io::Result<()> {
    let bytes = fs::read(from)?;
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    let mut file = options.open(to)?;
    let written = file.write_all(&bytes).and_then(|()| file.sync_all());
    drop(file);
    if let Err(error) = written {
        let _ = fs::remove_file(to);
        return Err(error);
    }
    #[cfg(unix)]
    sync_parent(to);
    Ok(())
}

/// Syncs `path`'s parent directory on Unix so directory entry updates (renames, creations) are durable across crashes.
#[cfg(unix)]
fn sync_parent(path: &Path) {
    if let Some(parent) = path.parent()
        && let Ok(dir) = fs::File::open(parent)
    {
        let _ = dir.sync_all();
    }
}

/// Writes `bytes` into a new `0o600` temporary file beside `target`, syncs it, and atomically renames it over `target`.
#[cfg(not(windows))]
fn write_private_atomic(target: &Path, bytes: &[u8]) -> io::Result<()> {
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent)?;
    }
    let temporary = PathBuf::from(format!("{}.{}.tmp", target.display(), new_id()));
    let written = (|| {
        {
            let mut options = fs::OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
            let mut file = options.open(&temporary)?;
            file.write_all(bytes)?;
            file.sync_all()?;
        }
        fs::rename(&temporary, target)
    })();
    if written.is_err() && temporary.exists() {
        let _ = fs::remove_file(&temporary);
    }
    written
}

/// Creates/refreshes `backup` from `existing` on Unix: when `existing` already carries owner-only `0o600` permissions (as every
/// `atomic_write` output does), attempts a zero-copy `hard_link` + atomic `rename`, falling back to `write_private_atomic` byte copy.
#[cfg(not(windows))]
fn copy_as_backup(existing: &Path, backup: &Path) -> io::Result<()> {
    let meta = match fs::metadata(existing) {
        Ok(meta) => meta,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if (meta.permissions().mode() & 0o777) == 0o600 {
            if let Some(parent) = backup.parent() {
                fs::create_dir_all(parent)?;
            }
            let backup_tmp = PathBuf::from(format!("{}.{}.tmp", backup.display(), new_id()));
            if fs::hard_link(existing, &backup_tmp).is_ok() {
                if fs::rename(&backup_tmp, backup).is_ok() {
                    sync_parent(backup);
                    return Ok(());
                }
                let _ = fs::remove_file(&backup_tmp);
            }
        }
    }
    #[cfg(not(unix))]
    let _ = meta;
    match fs::read(existing) {
        Ok(previous) => {
            write_private_atomic(backup, &previous)?;
            #[cfg(unix)]
            sync_parent(backup);
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    Ok(())
}

/// Crash-safe file replacement on Unix: if `destination` exists, refresh `backup` atomically via [`copy_as_backup`] (hard-linking an
/// already `0o600` file or copying bytes into a synced `0o600` temporary file, leaving `destination` untouched throughout), then perform
/// a single atomic `fs::rename(source, destination)` and `fsync` the parent directory so `destination` never disappears in any crash window.
#[cfg(not(windows))]
pub fn portable_replace(source: &Path, destination: &Path, backup: &Path) -> io::Result<()> {
    copy_as_backup(destination, backup)?;
    fs::rename(source, destination)?;
    #[cfg(unix)]
    sync_parent(destination);
    Ok(())
}

/// `File.Replace` as .NET does it on Windows (the host puts `ReplaceFileW` here on Windows).
#[cfg(windows)]
pub fn portable_replace(source: &Path, destination: &Path, backup: &Path) -> io::Result<()> {
    fs::rename(destination, backup)?;
    fs::rename(source, destination)
}

/// C# `StateStore.AtomicWriteAsync`: `data` written to a new temporary file beside `path` and flushed to the disk, then made the
/// file — with `backup`, the file it replaces kept there (`replace`), else moved over it. A refused swap (another reader holding
/// the file) is tried again, 40 times 50 ms apart; the temporary file never stays.
pub fn atomic_write(path: &Path, data: &[u8], backup: Option<&Path>, replace: fn(&Path, &Path, &Path) -> io::Result<()>) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let temporary = PathBuf::from(format!("{}.{}.tmp", path.display(), new_id()));
    let result = (|| {
        {
            let mut options = fs::OpenOptions::new();
            options.write(true).create_new(true);
            // The profile's files hold conversations and keys: the owner's alone on Unix.
            #[cfg(unix)]
            std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
            let mut file = options.open(&temporary)?;
            file.write_all(data)?;
            file.sync_all()?;
        }
        let mut attempt = 1;
        loop {
            let swapped = match backup {
                Some(backup) if path.exists() => replace(&temporary, path, backup),
                _ => {
                    let renamed = fs::rename(&temporary, path);
                    #[cfg(unix)]
                    if renamed.is_ok() {
                        sync_parent(path);
                    }
                    renamed
                }
            };
            match swapped {
                Ok(()) => return Ok(()),
                Err(_) if attempt < 40 => {
                    attempt += 1;
                    std::thread::sleep(Duration::from_millis(50));
                }
                Err(e) => return Err(e),
            }
        }
    })();
    if temporary.exists() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

/// C# `StateStore.SavedTheme`: the saved state's top-level `theme`, or "dark" (read before the state loads, for the splash).
pub fn saved_theme(directory: &Path) -> String {
    let path = directory.join(STATE_FILE);
    let Ok(meta) = fs::metadata(&path) else { return "dark".into() };
    if meta.len() > MAXIMUM_STATE_BYTES {
        return "dark".into();
    }
    let Ok(bytes) = fs::read(&path) else { return "dark".into() };
    // The first top-level `theme` (the reader stops there), when it is a string.
    let Ok(Value::Object(root)) = serde_json::from_slice::<Value>(bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(&bytes)) else {
        return "dark".into();
    };
    match root.get("theme") {
        Some(Value::String(theme)) => theme.clone(),
        _ => "dark".into(),
    }
}

/// When a save starts (C# `SaveSchedule`, M/StateRepository.swift): a short debounce after the change that asked for it, and at
/// least `minimum_gap` seconds after the last save began.
pub mod schedule {
    pub const DEBOUNCE: f64 = 0.22;
    pub const MINIMUM_GAP: f64 = 1.5;

    /// `SaveSchedule.Delay`.
    pub fn delay(since_last_start: Option<f64>) -> f64 {
        match since_last_start {
            Some(since) if since.is_finite() => DEBOUNCE.max(MINIMUM_GAP - since.max(0.0)),
            _ => DEBOUNCE,
        }
    }

    /// `SaveSchedule.KeepsPending`: a save already waiting stays as it is once the gap has passed since the last save began or since
    /// it was first asked for, so changes faster than the debounce never push it back for ever.
    pub fn keeps_pending(since_last_start: Option<f64>, since_first_request: f64) -> bool {
        since_last_start.is_some_and(|since| since.is_finite() && since >= MINIMUM_GAP)
            || since_first_request.is_finite() && since_first_request >= MINIMUM_GAP
    }
}

/// When the state is written (C# `DesktopService.QueueSave`, `ScheduleSave` and its save timer): saves run one after another on a
/// thread of their own, each writing the state it was queued with. A change the user made is queued at once ([`Saver::queue`]);
/// a stream of changes (a run's events, a draft) asks for one ([`Saver::schedule`]), which waits the debounce and at least the
/// gap since the last save began, then writes the state as it is by then. [`Saver::close`] writes the state one last time and
/// waits for every save.
pub struct Saver {
    shared: std::sync::Arc<(std::sync::Mutex<SaverState>, std::sync::Condvar)>,
    thread: std::sync::Mutex<Option<std::thread::JoinHandle<()>>>,
}

struct SaverState {
    queue: std::collections::VecDeque<AppSnapshot>,
    /// When the save now waiting was first asked for (`pendingSaveSince`).
    pending_since: Option<std::time::Instant>,
    /// When the latest save was queued (`lastSaveStart`).
    last_start: Option<std::time::Instant>,
    /// When the waiting save fires (the save timer).
    deadline: Option<std::time::Instant>,
    closing: bool,
    /// A save is being written now.
    writing: bool,
    saves: u64,
}

type Current = Box<dyn Fn() -> AppSnapshot + Send + Sync>;
type Write = Box<dyn FnMut(&AppSnapshot) -> Result<(), StateError> + Send>;
type Failed = Box<dyn Fn(&StateError) + Send + Sync>;

impl Saver {
    /// `current` gives the state as it is now (for a scheduled save when it fires and for the last save), `write` saves one state
    /// (the store's `save`), `failed` hears a save that did not go through (C#'s `PersistenceFailed`).
    pub fn new(
        current: impl Fn() -> AppSnapshot + Send + Sync + 'static,
        write: impl FnMut(&AppSnapshot) -> Result<(), StateError> + Send + 'static,
        failed: impl Fn(&StateError) + Send + Sync + 'static,
    ) -> Saver {
        let shared = std::sync::Arc::new((
            std::sync::Mutex::new(SaverState {
                queue: Default::default(),
                pending_since: None,
                last_start: None,
                deadline: None,
                closing: false,
                writing: false,
                saves: 0,
            }),
            std::sync::Condvar::new(),
        ));
        let (current, mut write, failed): (Current, Write, Failed) = (Box::new(current), Box::new(write), Box::new(failed));
        let worker = shared.clone();
        let thread = std::thread::Builder::new()
            .name("state-saver".into())
            .spawn(move || {
                let (lock, wake) = &*worker;
                let mut state = lock.lock().unwrap_or_else(|p| p.into_inner());
                loop {
                    if let Some(next) = state.queue.pop_front() {
                        state.writing = true;
                        drop(state);
                        if let Err(error) = write(&next) {
                            failed(&error);
                        }
                        state = lock.lock().unwrap_or_else(|p| p.into_inner());
                        state.writing = false;
                        state.saves += 1;
                        wake.notify_all();
                        continue;
                    }
                    if state.closing {
                        break;
                    }
                    match state.deadline {
                        Some(deadline) if deadline <= std::time::Instant::now() => {
                            state.deadline = None;
                            if state.pending_since.is_some() {
                                // The timer fired with a save still waiting: the state as it is now (QueueSave).
                                drop(state);
                                let snapshot = current();
                                state = lock.lock().unwrap_or_else(|p| p.into_inner());
                                if state.pending_since.is_some() {
                                    state.pending_since = None;
                                    state.last_start = Some(std::time::Instant::now());
                                    state.queue.push_back(snapshot);
                                }
                            }
                        }
                        Some(deadline) => {
                            let wait = deadline.saturating_duration_since(std::time::Instant::now());
                            state = wake.wait_timeout(state, wait).unwrap_or_else(|p| p.into_inner()).0;
                        }
                        None => state = wake.wait(state).unwrap_or_else(|p| p.into_inner()),
                    }
                }
            })
            .expect("the state saver thread");
        Saver { shared, thread: std::sync::Mutex::new(Some(thread)) }
    }

    fn state(&self) -> std::sync::MutexGuard<'_, SaverState> {
        self.shared.0.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// `QueueSave`: `snapshot` saved after every save queued before it.
    pub fn queue(&self, snapshot: AppSnapshot) {
        let mut state = self.state();
        state.pending_since = None;
        state.last_start = Some(std::time::Instant::now());
        state.queue.push_back(snapshot);
        self.shared.1.notify_all();
    }

    /// `ScheduleSave`: a save soon, which a waiting one already covers unless the gap passed (`SaveSchedule`).
    pub fn schedule(&self) {
        let mut state = self.state();
        if state.closing {
            return;
        }
        let now = std::time::Instant::now();
        let since = state.last_start.map(|last| now.duration_since(last).as_secs_f64());
        if let Some(waiting) = state.pending_since
            && schedule::keeps_pending(since, now.duration_since(waiting).as_secs_f64())
        {
            return;
        }
        state.pending_since.get_or_insert(now);
        state.deadline = Some(now + std::time::Duration::from_secs_f64(schedule::delay(since)));
        self.shared.1.notify_all();
    }

    /// How many saves have been written (tried) so far.
    pub fn saves(&self) -> u64 {
        self.state().saves
    }

    /// Waits until every queued save is written.
    pub fn flush(&self) {
        let mut state = self.state();
        while !state.queue.is_empty() || state.writing {
            state = self.shared.1.wait(state).unwrap_or_else(|p| p.into_inner());
        }
    }

    /// `DisposeAsync`'s end: `last` (the state now) saved after the others, then the thread ends. Later calls do nothing.
    pub fn close(&self, last: AppSnapshot) {
        {
            let mut state = self.state();
            if state.closing {
                return;
            }
            state.closing = true;
            state.pending_since = None;
            state.queue.push_back(last);
            self.shared.1.notify_all();
        }
        if let Some(thread) = self.thread.lock().unwrap_or_else(|p| p.into_inner()).take() {
            let _ = thread.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    type Written = std::sync::Arc<std::sync::Mutex<Vec<(std::time::Instant, String)>>>;

    fn recording() -> (Saver, Written) {
        let written = std::sync::Arc::new(std::sync::Mutex::new(vec![]));
        let sink = written.clone();
        let saver = Saver::new(
            || AppSnapshot { theme: Some("current".into()), ..AppSnapshot::default() },
            move |s: &AppSnapshot| {
                sink.lock().unwrap().push((std::time::Instant::now(), s.theme().to_string()));
                Ok(())
            },
            |_| {},
        );
        (saver, written)
    }

    fn journal_entry(id: &str, text: &str) -> LogEntry {
        LogEntry::new(id.to_string(), "assistant", text.to_string(), "2026-11-22T00:00:00Z".to_string(), None)
    }

    /// A replace record finds its entry by id: a base trimmed at the front (or an older `.bak`) puts it at another index than the
    /// writer's, and an entry the base does not hold is left out instead of landing on another one.
    #[test]
    fn a_journal_replace_finds_its_entry_by_id_not_by_the_writers_index() {
        let mut body = PaneBody {
            body_revision: 1,
            logs: vec![journal_entry("b", "b0"), journal_entry("c", "c0"), journal_entry("d", "d0")],
            ..PaneBody::default()
        };
        // The writer had [a, b, c, d]: `c` was at 2, `a` at 0, and `a` is gone from this base.
        body.apply(BodyJournalRecord {
            rev: 2,
            session_id: "s".into(),
            ops: vec![
                BodyJournalOp::LogReplaceLast { index: 2, entry: journal_entry("c", "c1") },
                BodyJournalOp::LogReplaceLast { index: 0, entry: journal_entry("a", "a1") },
                BodyJournalOp::LogReplaceLast { index: 9, entry: journal_entry("d", "d1") },
            ],
        });
        let texts: Vec<(&str, &str)> = body.logs.iter().map(|e| (e.id(), e.text())).collect();
        assert_eq!(texts, [("b", "b0"), ("c", "c1"), ("d", "d1")]);
        assert_eq!(body.body_revision, 2);
    }

    #[test]
    fn queued_saves_run_in_order_and_close_writes_the_last_state() {
        let (saver, written) = recording();
        saver.queue(AppSnapshot { theme: Some("one".into()), ..AppSnapshot::default() });
        saver.queue(AppSnapshot { theme: Some("two".into()), ..AppSnapshot::default() });
        saver.close(AppSnapshot { theme: Some("last".into()), ..AppSnapshot::default() });
        let themes: Vec<String> = written.lock().unwrap().iter().map(|(_, t)| t.clone()).collect();
        assert_eq!(themes, ["one", "two", "last"]);
    }

    #[test]
    fn a_scheduled_save_waits_the_debounce_and_the_gap_and_takes_the_state_then() {
        let (saver, written) = recording();
        let start = std::time::Instant::now();
        saver.schedule();
        saver.schedule();
        std::thread::sleep(std::time::Duration::from_millis(600));
        {
            let w = written.lock().unwrap();
            assert_eq!(w.len(), 1, "two asks within the debounce are one save");
            let after = w[0].0.duration_since(start).as_secs_f64();
            assert!((0.2..0.5).contains(&after), "the first save came after {after} s");
            assert_eq!(w[0].1, "current");
        }
        // The next one keeps the 1.5 s gap from the last save's start.
        saver.schedule();
        std::thread::sleep(std::time::Duration::from_millis(1500));
        let w = written.lock().unwrap();
        assert_eq!(w.len(), 2);
        let gap = w[1].0.duration_since(w[0].0).as_secs_f64();
        assert!(gap >= 1.45, "the second save came {gap} s after the first");
    }

    #[test]
    fn version_must_be_the_integer_one() {
        let en = Locale::new("en");
        assert!(read_state(br#"{"version":1,"workspaces":[],"sessions":[]}"#, &en).is_ok());
        assert!(read_state(br#"{"version":1.0,"workspaces":[],"sessions":[]}"#, &en).is_err());
        assert!(read_state(br#"{"version":2,"workspaces":[],"sessions":[]}"#, &en).is_err());
        assert!(read_state(br#"{"version":1,"workspaces":{},"sessions":[]}"#, &en).is_err());
        assert!(read_state(b"\xEF\xBB\xBF{\"version\":1,\"workspaces\":[],\"sessions\":[]}", &en).is_ok());
    }

    #[test]
    fn a_misspelled_workspaces_key_is_left_out() {
        let root = json!({"version": 1, "Workspaces": [{"id": "x"}], "workspaces": [{"id": "y"}], "sessions": []});
        let kept = kept_workspaces(root.as_object().unwrap());
        assert!(!kept.contains_key("Workspaces"));
        assert_eq!(kept["workspaces"], json!([{"id": "y"}]));
    }

    #[test]
    fn a_remote_version_of_two_point_zero_is_written_as_two() {
        let root =
            json!({"workspaces": [{"id": "a", "remote": {"version": 2.0, "connectionId": "c", "workspaceId": "w", "hostName": "h"}}]});
        let kept = kept_workspaces(root.as_object().unwrap());
        assert_eq!(kept["workspaces"][0]["remote"]["version"], json!(2));
        let bad = json!({"workspaces": [{"id": "a", "remote": {"version": 2, "connectionId": 5}}]});
        assert_eq!(kept_workspaces(bad.as_object().unwrap())["workspaces"], json!([]));
    }

    #[test]
    fn the_schedule_debounces_and_keeps_a_gap() {
        assert_eq!(schedule::delay(None), 0.22);
        assert!((schedule::delay(Some(0.5)) - 1.0).abs() < 1e-9);
        assert_eq!(schedule::delay(Some(3.0)), 0.22);
        assert!(schedule::keeps_pending(Some(2.0), 0.0));
        assert!(schedule::keeps_pending(Some(0.1), 1.6));
        assert!(!schedule::keeps_pending(None, 0.5));
    }
}
