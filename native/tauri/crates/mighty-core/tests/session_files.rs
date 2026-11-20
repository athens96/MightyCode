//! The split state (docs/superpowers/plans/2026-11-20-session-state-files.md, phase 1): an index and one body file per pane, written
//! only for the panes that changed, read back whole, a closed pane's file removed, a damaged body set aside without touching the
//! other panes, and an index an older app wrote merged with the bodies it did not know of.

mod common;

use std::fs;
use std::path::Path;
use std::sync::Arc;

use common::temp_dir;
use mighty_core::locale::Locale;
use mighty_core::model::{AppSnapshot, LogEntry, RunSession};
use mighty_core::store::{self, Store};
use serde_json::Value;

const P1: &str = "11111111-1111-4111-8111-111111111111";
const P2: &str = "22222222-2222-4222-8222-222222222222";

fn en() -> Locale {
    Locale::new("en")
}

fn entry(id: &str, text: &str) -> String {
    format!(r#"{{"id":"{id}","kind":"assistant","text":"{text}","timestamp":"2026-11-20T00:00:00Z"}}"#)
}

fn pane(id: &str, logs: &[String]) -> String {
    format!(
        r#"{{"id":"{id}","workspaceId":"w1","title":"T","kind":"claude","status":"idle","provider":"claude","logs":[{}],"createdAt":"2026-11-20T00:00:00Z","draft":"draft of {id}"}}"#,
        logs.join(",")
    )
}

/// A one-file state (what every version before the split wrote) with two panes.
fn one_file() -> String {
    let root = if cfg!(windows) { r"C:\\a" } else { "/a" };
    format!(
        r#"{{"version":1,"workspaces":[{{"id":"w1","name":"A","path":"{root}","createdAt":"2026-11-20T00:00:00Z"}}],"sessions":[{},{}]}}"#,
        pane(P1, &[entry("a1", "first"), entry("a2", "second")]),
        pane(P2, &[entry("b1", "other")])
    )
}

fn loaded(dir: &Path) -> (Store, AppSnapshot) {
    let mut store = Store::new(dir.to_path_buf(), None, true);
    let snapshot = store.load(&en()).unwrap();
    (store, snapshot)
}

fn body(dir: &Path, id: &str) -> Value {
    serde_json::from_slice(&fs::read(dir.join(store::SESSIONS_DIR).join(store::body_file_name(id))).unwrap()).unwrap()
}

fn texts(snapshot: &AppSnapshot, id: &str) -> Vec<String> {
    snapshot.session(id).unwrap().logs().iter().map(|l| l.text().to_string()).collect()
}

fn with_log(snapshot: &AppSnapshot, id: &str, log_id: &str, text: &str) -> AppSnapshot {
    let mut next = snapshot.clone();
    let at = next.sessions().iter().position(|s| s.id() == id).unwrap();
    let mut pane = (*next.sessions()[at]).clone();
    let mut logs = pane.logs().to_vec();
    logs.push(LogEntry {
        id: Some(log_id.into()),
        kind: Some("assistant".into()),
        text: Some(text.into()),
        timestamp: Some("2026-11-20T00:01:00Z".into()),
        ..LogEntry::default()
    });
    pane.logs = Some(logs);
    next.sessions_mut()[at] = Arc::new(pane);
    next
}

#[test]
fn a_state_is_saved_as_an_index_and_a_body_per_pane_and_reads_back_whole() {
    let dir = temp_dir("split-roundtrip");
    fs::write(dir.join(store::STATE_FILE), one_file()).unwrap();
    let (mut store, snapshot) = loaded(&dir);
    assert!(!store.is_split());
    store.save(&snapshot).unwrap();
    assert!(store.is_split());

    let index: Value = serde_json::from_slice(&fs::read(dir.join(store::STATE_FILE)).unwrap()).unwrap();
    assert_eq!(index["sessionFiles"], 1);
    for pane in index["sessions"].as_array().unwrap() {
        assert!(pane.get("logs").is_none(), "the index holds no body: {pane}");
        assert_eq!(pane["bodyRevision"], 1);
        assert!(pane["draft"].as_str().unwrap().starts_with("draft of"), "the draft stays in the index");
    }
    let first = body(&dir, P1);
    assert_eq!((first["version"].as_i64(), first["sessionId"].as_str(), first["bodyRevision"].as_i64()), (Some(1), Some(P1), Some(1)));
    assert_eq!(first["logs"].as_array().unwrap().len(), 2);

    let (again, reread) = loaded(&dir);
    assert!(again.is_split());
    assert_eq!(texts(&reread, P1), ["first", "second"]);
    assert_eq!(texts(&reread, P2), ["other"]);
    assert_eq!(reread.session(P1).unwrap().draft.as_deref(), Some(&*format!("draft of {P1}")));
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn only_the_panes_that_changed_are_written_again() {
    let dir = temp_dir("split-dirty");
    fs::write(dir.join(store::STATE_FILE), one_file()).unwrap();
    let (mut store, snapshot) = loaded(&dir);
    store.save(&snapshot).unwrap();
    let untouched = fs::read(dir.join(store::SESSIONS_DIR).join(store::body_file_name(P2))).unwrap();

    let next = with_log(&snapshot, P1, "a3", "third");
    store.save(&next).unwrap();
    assert_eq!(body(&dir, P1)["bodyRevision"], 2);
    assert_eq!(body(&dir, P1)["logs"].as_array().unwrap().len(), 3);
    assert_eq!(fs::read(dir.join(store::SESSIONS_DIR).join(store::body_file_name(P2))).unwrap(), untouched, "P2 was not written");

    // Saving the same state again writes no pane.
    store.save(&next).unwrap();
    assert_eq!(body(&dir, P1)["bodyRevision"], 2);
    let (_, reread) = loaded(&dir);
    assert_eq!(texts(&reread, P1), ["first", "second", "third"]);
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn a_closed_panes_body_file_goes_with_it() {
    let dir = temp_dir("split-closed");
    fs::write(dir.join(store::STATE_FILE), one_file()).unwrap();
    let (mut store, snapshot) = loaded(&dir);
    store.save(&snapshot).unwrap();
    let mut closed = snapshot.clone();
    closed.sessions_mut().retain(|s| s.id() != P2);
    store.save(&closed).unwrap();
    let folder = dir.join(store::SESSIONS_DIR);
    assert!(folder.join(store::body_file_name(P1)).exists());
    assert!(!folder.join(store::body_file_name(P2)).exists());
    assert!(!folder.join(format!("{}.bak", store::body_file_name(P2))).exists());
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn a_damaged_body_is_set_aside_and_only_its_pane_loses_or_restores_its_history() {
    let dir = temp_dir("split-damaged");
    fs::write(dir.join(store::STATE_FILE), one_file()).unwrap();
    let (mut store, snapshot) = loaded(&dir);
    store.save(&snapshot).unwrap();
    // A second save leaves the first body as P1's backup.
    store.save(&with_log(&snapshot, P1, "a3", "third")).unwrap();
    let folder = dir.join(store::SESSIONS_DIR);
    let file = folder.join(store::body_file_name(P1));

    fs::write(&file, b"{\"version\":1,\0\0").unwrap();
    let (restored, reread) = loaded(&dir);
    assert_eq!(texts(&reread, P1), ["first", "second"], "P1 came back from its backup");
    assert_eq!(texts(&reread, P2), ["other"], "P2 was not touched");
    assert!(restored.load_notice.as_deref().unwrap_or("").contains("backup"), "{:?}", restored.load_notice);
    let aside: Vec<String> = fs::read_dir(&folder)
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.contains(".unreadable-"))
        .collect();
    assert_eq!(aside.len(), 1, "the damaged file was kept aside");

    // Without a backup the pane opens without its history; the others keep theirs.
    fs::write(folder.join(store::body_file_name(P2)), b"not json").unwrap();
    fs::remove_file(folder.join(format!("{}.bak", store::body_file_name(P2)))).ok();
    let (empty, reread) = loaded(&dir);
    assert!(texts(&reread, P2).is_empty());
    assert!(empty.load_notice.is_some());
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn an_index_an_older_app_wrote_is_merged_with_the_bodies_it_did_not_know() {
    let dir = temp_dir("split-downgrade");
    fs::write(dir.join(store::STATE_FILE), one_file()).unwrap();
    let (mut store, snapshot) = loaded(&dir);
    store.save(&snapshot).unwrap();
    // The app went back to a version before the split: it read the index (no bodies), added an entry and wrote one file.
    let root = if cfg!(windows) { r"C:\\a" } else { "/a" };
    let older = format!(
        r#"{{"version":1,"workspaces":[{{"id":"w1","name":"A","path":"{root}","createdAt":"2026-11-20T00:00:00Z"}}],"sessions":[{},{}]}}"#,
        pane(P1, &[entry("a9", "written by the older app")]),
        pane(P2, &[])
    );
    fs::write(dir.join(store::STATE_FILE), older).unwrap();
    let (again, reread) = loaded(&dir);
    assert!(!again.is_split());
    assert_eq!(texts(&reread, P1), ["first", "second", "written by the older app"]);
    assert_eq!(texts(&reread, P2), ["other"]);
    fs::remove_dir_all(dir).unwrap();
}

#[cfg(unix)]
#[test]
fn the_panes_files_are_the_owners_alone() {
    use std::os::unix::fs::PermissionsExt;
    let dir = temp_dir("split-modes");
    fs::write(dir.join(store::STATE_FILE), one_file()).unwrap();
    let (mut store, snapshot) = loaded(&dir);
    store.save(&snapshot).unwrap();
    let mode = |path: &Path| fs::metadata(path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode(&dir.join(store::SESSIONS_DIR)), 0o700);
    assert_eq!(mode(&dir.join(store::SESSIONS_DIR).join(store::body_file_name(P1))), 0o600);
    assert_eq!(mode(&dir.join(store::STATE_FILE)), 0o600);
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn a_pane_id_that_is_no_safe_file_name_gets_a_hashed_one() {
    assert_eq!(store::body_file_name(P1), format!("{P1}.json"));
    let odd = store::body_file_name("files:w1");
    assert!(odd.starts_with("h-") && odd.ends_with(".json") && !odd.contains(':'), "{odd}");
    let _ = RunSession::default();
}
