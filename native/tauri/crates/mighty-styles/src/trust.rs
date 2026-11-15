//! The approvals (C# `StyleTrustStore` in `StyleRegistry.cs`): `<profile>\style-trust\approvals.json`, version 1, at most 256 records
//! and 4 MiB, written as a temporary file moved over it under `write.lock`, in a folder only this user may change; a file that cannot be
//! read as one locks the store (`settings.styles.lockBanner`). The owner-only checks go through [`TrustAccess`] (the host's is the
//! OS's; [`PlainAccess`] refuses links only). Owner: W9-D.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use mighty_core::locale::Locale;
use mighty_core::time::DateTimeOffset;
use serde_json::{Map, Value, json};

use crate::registry::RegisteredStyle;

/// How the store checks and protects its folder and file (C# `Private` and `Restrict`).
pub trait TrustAccess: Send + Sync {
    /// Not a link or other reparse point, and only this user may change it.
    fn private(&self, path: &Path, directory: bool) -> bool;
    /// Makes it this user's alone (the folder is created first by the store).
    fn restrict(&self, path: &Path, directory: bool) -> std::io::Result<()>;
    /// `write.lock` opened (made when missing) and held exclusively until the answer is dropped, as C#'s `FileShare.None` holds it;
    /// an error when another holds it. The default opens it without excluding anyone (the OS's own exclusion is the host's).
    fn lock(&self, path: &Path) -> std::io::Result<Box<dyn std::any::Any + Send>> {
        let file = std::fs::OpenOptions::new().read(true).write(true).create(true).truncate(false).open(path)?;
        Ok(Box::new(file))
    }
}

/// Refuses a link or junction (C#'s reparse-point check); writes no access list. The tests', and the fallback where the OS gives none.
#[derive(Debug, Default, Clone, Copy)]
pub struct PlainAccess;

impl TrustAccess for PlainAccess {
    fn private(&self, path: &Path, _directory: bool) -> bool {
        !std::fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink())
            && !matches!(mighty_core::workspace_files::link_target(path), Ok(Some(_)))
    }
    fn restrict(&self, _path: &Path, _directory: bool) -> std::io::Result<()> {
        Ok(())
    }
}

/// C# `StyleApprovalRecord`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StyleApprovalRecord {
    pub style_id: String,
    pub source: String,
    pub path: String,
    pub workspace_path: Option<String>,
    pub hash: String,
    pub state: String,
    pub decided_at: String,
}

/// `StyleTrustStore.SamePath`: case-blind on Windows, exact elsewhere.
pub fn same_path(a: Option<&str>, b: Option<&str>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(a), Some(b)) => {
            if std::path::MAIN_SEPARATOR == '\\' {
                a.to_uppercase() == b.to_uppercase()
            } else {
                a == b
            }
        }
        _ => false,
    }
}

fn same_place(r: &StyleApprovalRecord, s: &RegisteredStyle) -> bool {
    r.source == s.source && same_path(Some(&r.path), Some(&s.path)) && same_path(r.workspace_path.as_deref(), s.workspace_path.as_deref())
}

/// The error a store answers with: the locked store's banner, or the plain refusal C# throws.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrustError(pub String);

impl std::fmt::Display for TrustError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for TrustError {}

/// One writer at a time in this process (C#'s static `Gate`); `write.lock` keeps another process out.
static GATE: Mutex<()> = Mutex::new(());

/// C# `StyleTrustStore(directory)`.
#[derive(Clone)]
pub struct StyleTrustStore {
    pub directory: PathBuf,
    access: Arc<dyn TrustAccess>,
    locale: Locale,
}

impl std::fmt::Debug for StyleTrustStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StyleTrustStore").field("directory", &self.directory).finish_non_exhaustive()
    }
}

/// `.NET`'s DateTimeOffset as System.Text.Json writes it: the `"O"` text with the fraction's trailing zeros dropped.
fn json_time(time: DateTimeOffset) -> String {
    let o = time.to_o();
    match (o.find('.'), o.rfind(['+', '-'])) {
        (Some(dot), Some(sign)) if sign > dot => {
            let fraction = o[dot + 1..sign].trim_end_matches('0');
            if fraction.is_empty() { format!("{}{}", &o[..dot], &o[sign..]) } else { format!("{}.{fraction}{}", &o[..dot], &o[sign..]) }
        }
        _ => o,
    }
}

/// A field by name, case-blind (`JsonSerializerDefaults.Web`).
fn field<'a>(object: &'a Map<String, Value>, name: &str) -> Option<&'a Value> {
    object.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v)
}

fn optional_text(object: &Map<String, Value>, name: &str) -> Result<Option<String>, ()> {
    match field(object, name) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => Ok(Some(s.clone())),
        Some(_) => Err(()),
    }
}

impl StyleTrustStore {
    pub fn new(directory: &Path, access: Arc<dyn TrustAccess>, locale: Locale) -> Self {
        let directory = mighty_core::workspace_files::full_path(directory).unwrap_or_else(|| directory.to_path_buf());
        StyleTrustStore { directory, access, locale }
    }

    pub fn file_path(&self) -> PathBuf {
        self.directory.join("approvals.json")
    }

    fn locked(&self) -> TrustError {
        TrustError(self.locale.get("settings.styles.lockBanner"))
    }

    /// `Load`: the records, or the locked store's error.
    pub fn load(&self) -> Result<Vec<StyleApprovalRecord>, TrustError> {
        let file = self.file_path();
        if !file.is_file() {
            return Ok(Vec::new());
        }
        if !self.access.private(&self.directory, true) || !self.access.private(&file, false) {
            return Err(self.locked());
        }
        let bytes = crate::files::read(&self.directory, "approvals.json", 4 * 1024 * 1024).ok_or_else(|| self.locked())?;
        self.parse(&bytes).ok_or_else(|| self.locked())
    }

    fn parse(&self, bytes: &[u8]) -> Option<Vec<StyleApprovalRecord>> {
        let root: Value = serde_json::from_slice(bytes).ok()?;
        let object = root.as_object()?;
        let version = match field(object, "version") {
            Some(Value::Number(n)) => n.as_i64(),
            Some(Value::String(s)) => s.parse().ok(),
            _ => None,
        };
        if version != Some(1) {
            return None;
        }
        let records = field(object, "records")?.as_array()?;
        if records.len() > 256 {
            return None;
        }
        let mut out = Vec::new();
        for record in records {
            let r = record.as_object()?;
            let state = optional_text(r, "state").ok()??;
            let source = optional_text(r, "source").ok()??;
            let hash = optional_text(r, "hash").ok()??;
            if !matches!(state.as_str(), "approved" | "revoked")
                || !matches!(source.as_str(), "user" | "workspace")
                || hash.encode_utf16().count() != 64
            {
                return None;
            }
            let decided_at = match field(r, "decidedAt") {
                None | Some(Value::Null) => String::new(),
                Some(Value::String(s)) => {
                    mighty_core::time::parse(Some(s))?;
                    s.clone()
                }
                Some(_) => return None,
            };
            out.push(StyleApprovalRecord {
                style_id: optional_text(r, "styleId").ok()?.unwrap_or_default(),
                source,
                path: optional_text(r, "path").ok()?.unwrap_or_default(),
                workspace_path: optional_text(r, "workspacePath").ok()?,
                hash,
                state,
                decided_at,
            });
        }
        Some(out)
    }

    /// `State(style)`: preApproved for a bundled style; revoked when any record of its place is; approved for these bytes; else pending.
    pub fn state(&self, style: &RegisteredStyle) -> Result<&'static str, TrustError> {
        if style.source == "bundled" {
            return Ok("preApproved");
        }
        let records = self.load()?;
        let place: Vec<&StyleApprovalRecord> = records.iter().filter(|r| same_place(r, style)).collect();
        Ok(if place.iter().any(|r| r.state == "revoked") {
            "revoked"
        } else if place.iter().any(|r| r.state == "approved" && r.hash == style.hash) {
            "approved"
        } else {
            "pending"
        })
    }

    /// `Decide(style, state)`: `approved`, `revoked` or `unblock` (every decision of the place forgotten). The caller checks it may write.
    pub fn decide(&self, style: &RegisteredStyle, state: &str) -> Result<(), TrustError> {
        if style.source == "bundled" {
            return Ok(());
        }
        if !matches!(state, "approved" | "revoked" | "unblock") {
            return Err(TrustError("state".into()));
        }
        let _gate = GATE.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if self.directory.is_dir() && !self.access.private(&self.directory, true) {
            return Err(self.locked());
        }
        let io = |e: std::io::Error| TrustError(e.to_string());
        std::fs::create_dir_all(&self.directory).map_err(io)?;
        self.access.restrict(&self.directory, true).map_err(io)?;
        // Held for the whole decision, as C#'s `FileShare.None` holds it: another process's decision fails while this one runs.
        let _exclusive = self.access.lock(&self.directory.join("write.lock")).map_err(io)?;
        let mut records = self.load()?;
        if state == "approved" && records.iter().any(|r| same_place(r, style) && r.state == "revoked") {
            return Err(TrustError(self.locale.get("settings.styles.unblockButton")));
        }
        records.retain(|r| !(same_place(r, style) && (state != "approved" || r.state == "approved")));
        if state != "unblock" {
            while records.len() >= 256 {
                let oldest = records
                    .iter()
                    .enumerate()
                    .filter(|(_, r)| r.state == "approved")
                    .min_by_key(|(_, r)| mighty_core::time::parse(Some(&r.decided_at)).map(|t| t.utc_ticks()).unwrap_or(i64::MIN))
                    .map(|(i, _)| i)
                    .ok_or_else(|| TrustError("Style approval store is full.".into()))?;
                records.remove(oldest);
            }
            records.push(StyleApprovalRecord {
                style_id: style.id().to_string(),
                source: style.source.clone(),
                path: style.path.clone(),
                workspace_path: style.workspace_path.clone(),
                hash: style.hash.clone(),
                state: state.to_string(),
                decided_at: json_time(DateTimeOffset::now()),
            });
        }
        let temp = self.directory.join(format!("{}.tmp", uuid::Uuid::new_v4().simple()));
        let written = (|| {
            let mut output = std::fs::OpenOptions::new().write(true).create_new(true).open(&temp)?;
            output.write_all(&serialise(&records))?;
            output.sync_all()?;
            drop(output);
            self.access.restrict(&temp, false)?;
            std::fs::rename(&temp, self.file_path())
        })();
        let _ = std::fs::remove_file(&temp);
        written.map_err(io)
    }

    /// `Forget(style)`: every decision for the style's place goes, as removing its file does.
    pub fn forget(&self, style: &RegisteredStyle) -> Result<(), TrustError> {
        self.decide(style, "unblock")
    }
}

/// The store as C#'s `Wire.Json` writes it: compact, camel-case, a missing workspace left out.
fn serialise(records: &[StyleApprovalRecord]) -> Vec<u8> {
    let records: Vec<Value> = records
        .iter()
        .map(|r| {
            let mut o = Map::new();
            o.insert("styleId".into(), json!(r.style_id));
            o.insert("source".into(), json!(r.source));
            o.insert("path".into(), json!(r.path));
            if let Some(w) = &r.workspace_path {
                o.insert("workspacePath".into(), json!(w));
            }
            o.insert("hash".into(), json!(r.hash));
            o.insert("state".into(), json!(r.state));
            o.insert("decidedAt".into(), json!(if r.decided_at.is_empty() { "0001-01-01T00:00:00+00:00" } else { &r.decided_at }));
            Value::Object(o)
        })
        .collect();
    serde_json::to_vec(&json!({ "version": 1, "records": records })).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn times_are_written_as_system_text_json_writes_them() {
        let whole = json_time(DateTimeOffset::from_utc_ticks(638_000_000_000_000_000));
        assert!(whole.ends_with(":20+00:00") && !whole.contains('.'), "{whole}");
        let part = json_time(DateTimeOffset::from_utc_ticks(638_000_000_001_230_000));
        assert!(part.ends_with(":20.123+00:00"), "{part}");
    }
}
