//! Claude Code's own record of a session's task list: the task tools (TaskCreate, TaskUpdate) write each task to
//! `<config>/tasks/<session id>/<task id>.json` as `{id, subject, activeForm?, status}`. The checklist tracker follows the tools' calls
//! in the stream; a call it never saw (the app quit or updated during a run, a context compaction) leaves a task it holds stale, and a
//! stale unfinished task would come back as a step of every later request. The tracker checks the tasks it holds against this record
//! when a run starts and when its turn ends ([`crate::plan_mode::TodoProgressTracker::reconcile`]).

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::{Path, PathBuf};

use mighty_core::wire;
use serde_json::Value;

/// A task file larger than this is not one the task tools wrote.
const MAXIMUM_TASK_BYTES: u64 = 64 * 1024;

/// One task as Claude Code recorded it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordedTask {
    pub subject: Option<String>,
    pub active_form: Option<String>,
    pub status: String,
}

/// Claude's config folder for a run's environment: `CLAUDE_CONFIG_DIR` (taken against `home` when relative), else `<home>/.claude`.
pub fn config_folder(environment: &BTreeMap<String, String>, home: &Path) -> PathBuf {
    match environment.get("CLAUDE_CONFIG_DIR").filter(|dir| !dir.trim().is_empty()) {
        Some(dir) => home.join(dir),
        None => home.join(".claude"),
    }
}

/// The home folder a run's environment names (`USERPROFILE` on Windows, `HOME` elsewhere), else this process's.
pub fn home_of(environment: &BTreeMap<String, String>) -> Option<PathBuf> {
    let key = if crate::os::windows() { "USERPROFILE" } else { "HOME" };
    environment.get(key).cloned().or_else(|| std::env::var(key).ok()).filter(|home| !home.trim().is_empty()).map(PathBuf::from)
}

/// The folder holding `session_id`'s tasks; none for an id that could name another folder.
pub fn folder(config: &Path, session_id: &str) -> Option<PathBuf> {
    wire::session_identifier(session_id).then(|| config.join("tasks").join(session_id))
}

/// A task id the tools give: a short run of letters, digits, `-` and `_` (never a path).
fn task_id(id: &str) -> bool {
    (1..=64).contains(&id.len()) && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// The recorded tasks among `ids`: each found file's task by id; an id whose file is gone is left out (the task was deleted). None
/// when the folder does not exist or holds none of `ids` (another list, or a record this Claude Code does not keep): nothing to check.
pub fn read(folder: &Path, ids: &[String]) -> Option<HashMap<String, RecordedTask>> {
    if !folder.is_dir() {
        return None;
    }
    let mut found = HashMap::new();
    for id in ids.iter().filter(|id| task_id(id)) {
        let path = folder.join(format!("{id}.json"));
        let Ok(meta) = fs::metadata(&path) else { continue };
        if !meta.is_file() || meta.len() > MAXIMUM_TASK_BYTES {
            continue;
        }
        let Some(task) = fs::read(&path).ok().and_then(|bytes| parse(&bytes, id)) else { continue };
        found.insert(id.clone(), task);
    }
    (!found.is_empty()).then_some(found)
}

/// One task file's task, when it is one for `id` with a status the checklist knows.
pub fn parse(bytes: &[u8], id: &str) -> Option<RecordedTask> {
    let value: Value = serde_json::from_slice(bytes).ok()?;
    let recorded_id = match value.get("id")? {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        _ => return None,
    };
    if recorded_id != id {
        return None;
    }
    let status = value.get("status").and_then(Value::as_str)?;
    if !matches!(status, "pending" | "in_progress" | "completed") {
        return None;
    }
    let text = |key: &str| value.get(key).and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty()).map(str::to_string);
    Some(RecordedTask { subject: text("subject"), active_form: text("activeForm"), status: status.to_string() })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_task_file_reads_only_for_its_own_id_and_a_known_status() {
        let task = parse(br#"{"id":"313","subject":"0.5.5 release","status":"completed","blocks":[]}"#, "313").unwrap();
        assert_eq!(task, RecordedTask { subject: Some("0.5.5 release".into()), active_form: None, status: "completed".into() });
        assert!(parse(br#"{"id":"12","subject":"x","status":"completed"}"#, "13").is_none());
        assert!(parse(br#"{"id":"12","subject":"x","status":"deleted"}"#, "12").is_none());
        assert_eq!(
            parse(br#"{"id":12,"subject":"x","activeForm":"Doing x","status":"pending"}"#, "12").unwrap().active_form.as_deref(),
            Some("Doing x")
        );
    }

    #[test]
    fn the_folder_is_the_sessions_under_the_config_and_never_another_path() {
        let home = Path::new("/home/u");
        let mut environment = BTreeMap::new();
        assert_eq!(config_folder(&environment, home), Path::new("/home/u/.claude"));
        environment.insert("CLAUDE_CONFIG_DIR".to_string(), "/elsewhere/claude".to_string());
        assert_eq!(config_folder(&environment, home), Path::new("/elsewhere/claude"));
        let config = Path::new("/c");
        assert_eq!(
            folder(config, "6e08c782-9a5e-4eef-82de-a2a724f149ee").unwrap(),
            Path::new("/c/tasks/6e08c782-9a5e-4eef-82de-a2a724f149ee")
        );
        assert!(folder(config, "../x").is_none());
    }

    #[test]
    fn a_missing_file_reads_as_deleted_and_a_folder_with_none_of_the_ids_as_nothing_to_check() {
        let dir = std::env::temp_dir().join(format!("mc-claude-tasks-{}", wire::new_id()));
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("1.json"), br#"{"id":"1","subject":"a","status":"completed"}"#).unwrap();
        let ids = vec!["1".to_string(), "2".to_string(), "../3".to_string()];
        let found = read(&dir, &ids).unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found["1"].status, "completed");
        assert!(read(&dir, &["9".to_string()]).is_none());
        assert!(read(&dir.join("missing"), &ids).is_none());
        fs::remove_dir_all(&dir).unwrap();
    }
}
