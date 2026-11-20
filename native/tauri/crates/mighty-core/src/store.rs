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

use crate::model::{AppSnapshot, RemoteWorkspaceReference, RunSession, Workspace};
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

/// Removes the body files (and their `.bak`) of panes the index no longer lists: a closed pane's data goes as it always did. A file
/// set aside as unreadable stays for the person to look at.
fn remove_unlisted_bodies(folder: &Path, listed: &HashSet<String>) {
    let Ok(entries) = fs::read_dir(folder) else { return };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        let body = name.strip_suffix(".bak").unwrap_or(&name);
        if body.ends_with(".json") && !listed.contains(body) {
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
    /// Each pane's normalized state as its body was last written: a save writes a pane's file only when its pane is another object.
    written: HashMap<String, Arc<RunSession>>,
    /// The index bytes last written (an unchanged index is not written again).
    written_index: Option<Vec<u8>>,
    /// The panes a save normalized last, taken again while they are the same objects.
    cache: normalize::PaneCache,
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
            written: HashMap::new(),
            written_index: None,
            cache: normalize::PaneCache::default(),
        }
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
        self.written.clear();
        self.written_index = None;
        let mut notes = vec![];
        if let Value::Object(root) = &mut value {
            self.split = root.remove(crate::model::SPLIT_MARKER).and_then(|v| v.as_i64()) == Some(1);
            if let Some(Value::Array(panes)) = root.get_mut("sessions") {
                for pane in panes.iter_mut() {
                    let Some(entry) = pane.as_object_mut() else { continue };
                    let revision = entry.remove("bodyRevision").and_then(|v| v.as_u64());
                    let Some(id) = entry.get("id").and_then(Value::as_str).map(str::to_string) else { continue };
                    if let Some(revision) = revision {
                        self.revisions.insert(id.clone(), revision);
                    }
                    if let Some((body, revision)) = self.read_body(&id, locale, &mut notes) {
                        let known = self.revisions.get(&id).copied().unwrap_or(0);
                        self.revisions.insert(id.clone(), known.max(revision));
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
        read_state_value(value, locale)
    }

    /// `id`'s body file, its `.bak` when the file does not read (the file moved aside first, with writing on); none when it has none.
    fn read_body(&self, id: &str, locale: &Locale, notes: &mut Vec<String>) -> Option<(Map<String, Value>, u64)> {
        let folder = self.sessions_dir();
        let name = body_file_name(id);
        let path = folder.join(&name);
        if !path.exists() {
            return None;
        }
        match read_body_file(&path, id) {
            Ok(body) => Some(body),
            Err(_) => {
                let aside = format!("{name}.unreadable-{}-{}", chrono::Local::now().format("%Y%m%d-%H%M%S"), &new_id()[..8]);
                if self.writes {
                    let _ = fs::rename(&path, folder.join(&aside));
                }
                let restored = read_body_file(&folder.join(format!("{name}.bak")), id).ok();
                let key = if restored.is_some() { "store.notice.paneRestoredBackup" } else { "store.notice.paneStartedEmpty" };
                notes.push(locale.get_with(key, &[("file", &format!("{SESSIONS_DIR}/{aside}"))]));
                restored
            }
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
            fs::create_dir_all(&self.directory)?;
        }
        let state = self.state_path();
        let mut path = state.clone();
        self.source = Some(LoadSource::Empty);
        if !state.exists()
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
        if path.exists() {
            let bytes = fs::read(&path)?;
            if damaged(&bytes) {
                let aside = format!("{STATE_FILE}.unreadable-{}-{}", chrono::Local::now().format("%Y%m%d-%H%M%S"), &new_id()[..8]);
                if self.writes {
                    fs::rename(&state, self.directory.join(&aside))?;
                }
                let backup = self.backup_path();
                let restored = if backup.exists() {
                    match fs::read(&backup).map_err(StateError::Io).and_then(|b| self.read_split(&b, locale)) {
                        Ok(snapshot) => Some(snapshot),
                        Err(StateError::Io(e)) => return Err(StateError::Io(e)),
                        Err(StateError::Json(_) | StateError::InvalidFormat(_) | StateError::TooLarge(_)) => None,
                        Err(other) => return Err(other),
                    }
                } else {
                    None
                };
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
        Ok(clone_state(&self.snapshot))
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
        self.approved.get(workspace.id()).is_some_and(|original| original.path == workspace.path && original.remote == workspace.remote)
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

    /// C# `SaveAsync`: normalized, checked, written, as the split state: the body file of each pane that changed since its last
    /// write, then the index, then the body files of panes no longer listed removed. A crash between leaves bodies newer than the
    /// index, which loads. The normalized state is kept as the store's.
    pub fn save(&mut self, snapshot: &AppSnapshot) -> Result<(), StateError> {
        if !self.loaded {
            return Err(StateError::NotLoaded);
        }
        // The remote mirror's workspaces and panes are another computer's, held while connected: never written.
        let normalized = normalize::normalize_with(&snapshot.without_mirrors(), false, Some(&mut self.cache)).map_err(state_error)?;
        for workspace in normalized.workspaces() {
            if !self.approves(workspace) {
                return Err(StateError::PathNotApproved);
            }
        }
        let stored = normalize::stored(&normalized);
        let mut bodies: Vec<(Arc<RunSession>, u64, Vec<u8>)> = vec![];
        for pane in stored.sessions() {
            if self.written.get(pane.id()).is_some_and(|last| Arc::ptr_eq(last, pane)) {
                continue;
            }
            let revision = self.revisions.get(pane.id()).copied().unwrap_or(0) + 1;
            bodies.push((pane.clone(), revision, encode_body(pane, revision)?));
        }
        let fresh: HashMap<&str, u64> = bodies.iter().map(|(pane, revision, _)| (pane.id(), *revision)).collect();
        let index = encode_index(&stored, &|id| fresh.get(id).or_else(|| self.revisions.get(id)).copied().unwrap_or(0));
        if index.len() as u64 > MAXIMUM_STATE_BYTES {
            return Err(StateError::StateTooLarge(index.len()));
        }
        if !self.writes {
            return Err(StateError::WritesOff);
        }
        let state = self.state_path();
        if !self.split && state.exists() {
            // The one file of a state before the split, kept once as it was.
            let kept = self.directory.join(format!("{STATE_FILE}.before-session-files-{}", chrono::Local::now().format("%Y%m%d-%H%M%S")));
            if !kept.exists() {
                fs::copy(&state, &kept)?;
            }
        }
        let folder = self.sessions_dir();
        if !bodies.is_empty() || !folder.exists() {
            private_folder(&folder)?;
        }
        for (pane, revision, bytes) in bodies {
            let name = body_file_name(pane.id());
            atomic_write(&folder.join(&name), &bytes, Some(&folder.join(format!("{name}.bak"))), self.replace)?;
            self.revisions.insert(pane.id().to_string(), revision);
            self.written.insert(pane.id().to_string(), pane);
        }
        if self.written_index.as_deref() != Some(index.as_slice()) || !state.exists() {
            atomic_write(&state, &index, Some(&self.backup_path()), self.replace)?;
            self.written_index = Some(index);
        }
        self.split = true;
        let listed: HashSet<String> = stored.sessions().iter().map(|s| body_file_name(s.id())).collect();
        self.written.retain(|id, _| listed.contains(&body_file_name(id)));
        self.revisions.retain(|id, _| listed.contains(&body_file_name(id)));
        remove_unlisted_bodies(&folder, &listed);
        self.snapshot = normalized;
        Ok(())
    }
}

/// `File.Copy(from, to, overwrite: false)`.
fn copy_new(from: &Path, to: &Path) -> io::Result<()> {
    let bytes = fs::read(from)?;
    let mut file = fs::OpenOptions::new().write(true).create_new(true).open(to)?;
    file.write_all(&bytes)
}

/// `File.Replace` as .NET does it off Windows: the old file renamed to the backup, the new one renamed into its place. The host
/// puts `ReplaceFileW` here on Windows.
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
                _ => fs::rename(&temporary, path),
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
