//! The split state (docs/superpowers/plans/2026-11-20-session-state-files.md, phase 1): an index and one body file per pane, written
//! only for the panes that changed, read back whole, a closed pane's file removed, a damaged body set aside without touching the
//! other panes, and an index an older app wrote merged with the bodies it did not know of.

mod common;

use std::fs;
use std::path::Path;
use std::sync::Arc;

use common::temp_dir;
use mighty_core::locale::Locale;
use mighty_core::model::{AppSnapshot, LogEntry, RunSession, SessionDigest};
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

#[test]
fn the_index_keeps_a_summary_of_each_panes_body() {
    let dir = temp_dir("split-digest");
    fs::write(dir.join(store::STATE_FILE), one_file()).unwrap();
    let (mut store, snapshot) = loaded(&dir);
    store.save(&snapshot).unwrap();
    let index: Value = serde_json::from_slice(&fs::read(dir.join(store::STATE_FILE)).unwrap()).unwrap();
    let first = index["sessions"].as_array().unwrap().iter().find(|p| p["id"] == P1).unwrap();
    assert_eq!(first["digest"]["logCount"], 2);
    assert_eq!(first["digest"]["tail"].as_array().unwrap().len(), 2);

    let (again, _) = loaded(&dir);
    let digest = again.index_digest(P1).expect("the summary read with the index");
    assert_eq!(digest.log_count, 2);
    assert_eq!(digest.tail.last().unwrap().text(), "second");
    assert_eq!(again.body_revision(P1), 1);
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn a_pane_whose_body_is_on_disk_answers_from_its_summary() {
    let long = "x".repeat(5000);
    let mut pane = RunSession { id: Some(P1.into()), ..RunSession::default() };
    pane.logs = Some(
        (0..20)
            .map(|i| LogEntry {
                id: Some(format!("e{i}")),
                kind: Some("assistant".into()),
                text: Some(long.clone()),
                ..LogEntry::default()
            })
            .collect(),
    );
    let digest = SessionDigest::of(&pane);
    assert_eq!(digest.log_count, 20);
    assert_eq!(digest.tail.len(), mighty_core::model::DIGEST_TAIL);
    assert!(digest.tail.iter().all(|e| e.text().len() <= mighty_core::model::DIGEST_TEXT));
    let on_disk = RunSession {
        logs: Some(digest.tail.clone()),
        graph_runs: None,
        body_on_disk: true,
        stored_digest: Some(Arc::new(SessionDigest { run_ids: vec!["run-1".into()], ..digest })),
        ..pane
    };
    assert_eq!(on_disk.log_count(), 20);
    assert_eq!(on_disk.kept_run_ids(), ["run-1"]);
    assert_eq!(on_disk.body_digest().log_count, 20);
}

#[test]
fn a_pane_normalized_again_into_another_object_with_the_same_body_is_not_written() {
    let dir = temp_dir("split-same-body");
    fs::write(dir.join(store::STATE_FILE), one_file()).unwrap();
    let (mut store, snapshot) = loaded(&dir);
    store.save(&snapshot).unwrap();
    // What the window's every change does: the whole state normalized, every pane a new object; one pane's draft changes.
    let mut next = mighty_core::normalize::normalize(&snapshot, false).unwrap();
    let at = next.sessions().iter().position(|s| s.id() == P2).unwrap();
    next.sessions_mut()[at] = Arc::new(RunSession { draft: Some("typed".into()), ..(*next.sessions()[at]).clone() });
    store.save(&next).unwrap();
    assert_eq!(body(&dir, P1)["bodyRevision"], 1, "P1's body did not change");
    assert_eq!(body(&dir, P2)["bodyRevision"], 1, "a new draft is the index's, not the body's");
    let (_, reread) = loaded(&dir);
    assert_eq!(reread.session(P2).unwrap().draft.as_deref(), Some("typed"));
    fs::remove_dir_all(dir).unwrap();
}

const P3: &str = "33333333-3333-4333-8333-333333333333";

/// Two workspaces, the first active; the second's pane has a long history whose latest request is older than its last entries.
fn two_workspaces() -> String {
    let (a, b) = if cfg!(windows) { (r"C:\\a", r"C:\\b") } else { ("/a", "/b") };
    let mut logs = vec![r#"{"id":"u1","kind":"user","text":"make the tests pass","timestamp":"2026-11-20T00:00:00Z"}"#.to_string()];
    logs.extend((0..12).map(|i| entry(&format!("c{i}"), &format!("reply {i}"))));
    format!(
        r#"{{"version":1,"workspaces":[{{"id":"w1","name":"A","path":"{a}","createdAt":"2026-11-20T00:00:00Z"}},{{"id":"w2","name":"B","path":"{b}","createdAt":"2026-11-20T00:00:00Z"}}],"activeWorkspaceId":"w1","sessions":[{},{}]}}"#,
        pane(P1, &[entry("a1", "first")]),
        pane(P3, &logs).replace(r#""workspaceId":"w1""#, r#""workspaceId":"w2""#)
    )
}

fn lazy(dir: &Path) -> (Store, AppSnapshot) {
    let mut store = Store::new(dir.to_path_buf(), None, true);
    store.lazy = true;
    let snapshot = store.load(&en()).unwrap();
    (store, snapshot)
}

#[test]
fn a_lazy_load_reads_the_active_workspaces_bodies_and_leaves_the_others_on_disk() {
    let dir = temp_dir("split-lazy");
    fs::write(dir.join(store::STATE_FILE), two_workspaces()).unwrap();
    let (mut first, snapshot) = loaded(&dir);
    store_split(&mut first, &snapshot);
    let eager_title = loaded(&dir).1.session(P3).unwrap().title().to_string();

    let (mut store, snapshot) = lazy(&dir);
    let active = snapshot.session(P1).unwrap();
    assert!(!active.body_on_disk);
    let other = snapshot.session(P3).unwrap();
    assert!(other.body_on_disk, "the other workspace's pane keeps its body on disk");
    assert_eq!(other.log_count(), 13);
    assert!(other.logs().len() < 13, "it holds its summary's entries");
    assert_eq!(other.logs().last().unwrap().text(), "reply 11");
    assert_eq!(other.title(), eager_title, "titled by its latest request as a whole load titles it");

    let whole = store.load_pane_body(other).expect("its body");
    assert!(!whole.body_on_disk);
    assert_eq!(whole.logs().len(), 13);
    assert_eq!(whole.title(), eager_title);

    // A save of the state as it loaded writes no body, the one on disk included.
    store.save(&snapshot).unwrap();
    assert_eq!(body(&dir, P3)["bodyRevision"], 1);
    assert_eq!(body(&dir, P3)["logs"].as_array().unwrap().len(), 13);
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn something_added_to_a_pane_whose_body_is_on_disk_is_laid_over_its_file_never_in_its_place() {
    let dir = temp_dir("split-lazy-added");
    fs::write(dir.join(store::STATE_FILE), two_workspaces()).unwrap();
    let (mut first, snapshot) = loaded(&dir);
    store_split(&mut first, &snapshot);

    let (mut store, snapshot) = lazy(&dir);
    assert!(snapshot.session(P3).unwrap().body_on_disk);
    let next = with_log(&snapshot, P3, "note", "a note added before the body was read");
    store.save(&next).unwrap();
    let file = body(&dir, P3);
    assert_eq!(file["bodyRevision"], 2);
    assert_eq!(file["logs"].as_array().unwrap().len(), 14, "the 13 on disk and the note");
    // Saved again unchanged: not written again.
    store.save(&next).unwrap();
    assert_eq!(body(&dir, P3)["bodyRevision"], 2);
    let (_, reread) = loaded(&dir);
    assert_eq!(reread.session(P3).unwrap().logs().len(), 14);
    assert_eq!(reread.session(P3).unwrap().logs().last().unwrap().text(), "a note added before the body was read");
    fs::remove_dir_all(dir).unwrap();
}

fn store_split(store: &mut Store, snapshot: &AppSnapshot) {
    store.save(snapshot).unwrap();
    assert!(store.is_split());
}

#[test]
fn a_copy_of_the_state_that_lost_a_panes_mark_never_overwrites_its_body() {
    let dir = temp_dir("split-lazy-lost-mark");
    fs::write(dir.join(store::STATE_FILE), two_workspaces()).unwrap();
    let (mut first, snapshot) = loaded(&dir);
    store_split(&mut first, &snapshot);

    let (mut store, snapshot) = lazy(&dir);
    // The state written and read back somewhere in the app: the mark and the summary are gone, the summary's entries remain.
    let copy = store::clone_state(&snapshot);
    assert!(!copy.session(P3).unwrap().body_on_disk);
    store.save(&copy).unwrap();
    let (_, reread) = loaded(&dir);
    assert_eq!(reread.session(P3).unwrap().logs().len(), 13, "the whole history is still there");
    fs::remove_dir_all(dir).unwrap();
}
