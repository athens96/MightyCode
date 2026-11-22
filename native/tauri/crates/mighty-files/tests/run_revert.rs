//! 요청 되돌리기 (revert.rs, Swift RunRevertTests): what a request changed goes back to the originals the store kept from its edits,
//! and nothing a later run or the folder rules protect is written.

mod common;

use std::path::Path;

use common::{Temp, write};
use mighty_files::changes::{self, FileOriginalStore};
use mighty_files::patch::FileEdit;
use mighty_files::revert::{self, Action, Outcome, Refusal};

fn setup(name: &str) -> (Temp, String, FileOriginalStore) {
    let base = Temp::new(name);
    let root = base.path().join("work");
    std::fs::create_dir_all(&root).unwrap();
    let store = FileOriginalStore::new(base.path().join("originals"), changes::MAXIMUM_BYTES, common::handles());
    let root = root.to_string_lossy().into_owned();
    (base, root, store)
}

fn capture(store: &FileOriginalStore, run: &str, root: &str, relative: &str, original: Option<&str>) -> bool {
    let path = Path::new(root).join(relative.replace('/', std::path::MAIN_SEPARATOR_STR));
    store.capture("pane", run, root, &path.to_string_lossy(), &FileEdit::text(original))
}

fn read(root: &str, relative: &str) -> Option<Vec<u8>> {
    std::fs::read(Path::new(root).join(relative.replace('/', std::path::MAIN_SEPARATOR_STR))).ok()
}

fn remove(path: &Path) -> Result<(), String> {
    std::fs::remove_file(path).map_err(|e| e.to_string())
}

#[test]
fn a_runs_edits_go_back_and_its_new_files_go_to_the_trash() {
    let (_base, root, store) = setup("revert-run");
    write(Path::new(&root), "a.txt", b"one\nTWO\nthree\n");
    assert!(capture(&store, "run1", &root, "a.txt", Some("one\ntwo\n")));
    write(Path::new(&root), "src/b.txt", b"new\n");
    assert!(capture(&store, "run1", &root, "src/b.txt", None));

    let plan = revert::plan(&store, "pane", "run1", &root);
    assert_eq!(plan.iter().map(|i| i.path.as_str()).collect::<Vec<_>>(), ["a.txt", "src/b.txt"]);
    assert!(plan[0].action == Action::Restore && plan[0].restored == 1 && plan[0].removed == 2 && plan[0].revertible(), "{:?}", plan[0]);
    assert!(plan[1].action == Action::Trash && plan[1].revertible(), "{:?}", plan[1]);

    let trashed = std::sync::Mutex::new(Vec::new());
    let outcomes = revert::apply(&store, "pane", "run1", &root, None, &|path| {
        trashed.lock().unwrap().push(path.file_name().unwrap().to_string_lossy().into_owned());
        remove(path)
    });
    assert_eq!(outcomes, [("a.txt".to_string(), Outcome::Reverted), ("src/b.txt".to_string(), Outcome::Reverted)]);
    assert_eq!(read(&root, "a.txt").as_deref(), Some(&b"one\ntwo\n"[..]));
    assert_eq!(*trashed.lock().unwrap(), ["b.txt"]);
    assert!(read(&root, "src/b.txt").is_none());
    // Done once, the same revert finds nothing left to undo: the restored file reads as its original, the trashed one is gone.
    let again = revert::plan(&store, "pane", "run1", &root);
    assert_eq!(again.iter().map(|i| i.refusal).collect::<Vec<_>>(), [Some(Refusal::Unchanged), Some(Refusal::Missing)]);
}

#[test]
fn a_path_a_later_run_changed_is_refused_until_that_run_goes_first() {
    let (_base, root, store) = setup("revert-later");
    write(Path::new(&root), "a.txt", b"v1\n");
    capture(&store, "run1", &root, "a.txt", Some("v0\n"));
    capture(&store, "run2", &root, "a.txt", Some("v1\n"));
    write(Path::new(&root), "a.txt", b"v2\n");

    assert_eq!(revert::plan(&store, "pane", "run1", &root)[0].refusal, Some(Refusal::LaterRun));
    assert_eq!(revert::apply(&store, "pane", "run1", &root, None, &remove), [("a.txt".to_string(), Outcome::Refused(Refusal::LaterRun))]);
    assert_eq!(read(&root, "a.txt").as_deref(), Some(&b"v2\n"[..]));

    assert_eq!(revert::apply(&store, "pane", "run2", &root, None, &remove), [("a.txt".to_string(), Outcome::Reverted)]);
    assert_eq!(read(&root, "a.txt").as_deref(), Some(&b"v1\n"[..]));
}

#[test]
fn only_the_chosen_paths_are_written_and_a_missing_file_is_refused() {
    let (_base, root, store) = setup("revert-chosen");
    write(Path::new(&root), "a.txt", b"x\n");
    write(Path::new(&root), "b.txt", b"y\n");
    capture(&store, "run1", &root, "a.txt", Some("a0\n"));
    capture(&store, "run1", &root, "b.txt", Some("b0\n"));
    let chosen = ["b.txt".to_string()];
    assert_eq!(revert::apply(&store, "pane", "run1", &root, Some(&chosen), &remove), [("b.txt".to_string(), Outcome::Reverted)]);
    assert_eq!(read(&root, "a.txt").as_deref(), Some(&b"x\n"[..]));
    assert_eq!(read(&root, "b.txt").as_deref(), Some(&b"b0\n"[..]));

    std::fs::remove_file(Path::new(&root).join("a.txt")).unwrap();
    assert_eq!(revert::plan(&store, "pane", "run1", &root)[0].refusal, Some(Refusal::Missing));
    assert!(read(&root, "a.txt").is_none());
}

/// The original's line endings come back as they were (a Windows file stays CRLF), in the encoding and with the mark the file has now.
#[test]
fn the_file_keeps_its_encoding_mark_and_the_originals_line_endings() {
    let (_base, root, store) = setup("revert-encoding");
    write(Path::new(&root), "tool.cmd", b"\xEF\xBB\xBFecho new\r\n");
    capture(&store, "run1", &root, "tool.cmd", Some("\u{FEFF}echo old\r\nexit\r\n"));
    write(Path::new(&root), "notes.txt", &[0xFF, 0xFE, b'n', 0, b'\n', 0]);
    capture(&store, "run1", &root, "notes.txt", Some("old\n"));
    let outcomes = revert::apply(&store, "pane", "run1", &root, None, &remove);
    assert!(outcomes.iter().all(|(_, o)| *o == Outcome::Reverted), "{outcomes:?}");
    assert_eq!(read(&root, "tool.cmd").unwrap(), b"\xEF\xBB\xBFecho old\r\nexit\r\n");
    assert_eq!(read(&root, "notes.txt").unwrap(), [0xFF, 0xFE, b'o', 0, b'l', 0, b'd', 0, b'\n', 0]);
}

#[cfg(unix)]
#[test]
fn the_file_keeps_its_permissions() {
    use std::os::unix::fs::PermissionsExt as _;
    let (_base, root, store) = setup("revert-mode");
    let path = write(Path::new(&root), "tool.sh", b"echo new\n");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    capture(&store, "run1", &root, "tool.sh", Some("echo old\n"));
    assert_eq!(revert::apply(&store, "pane", "run1", &root, None, &remove), [("tool.sh".to_string(), Outcome::Reverted)]);
    assert_eq!(std::fs::read(&path).unwrap(), b"echo old\n");
    assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o755);
}

/// A run's refused paths are no files it kept: the review lists only what it can show and put back, under the folder it was kept for.
#[test]
fn the_kept_files_leave_out_refused_paths_and_other_folders() {
    let (base, root, store) = setup("revert-kept");
    write(Path::new(&root), "a.txt", b"now\n");
    write(Path::new(&root), "b.txt", b"now\n");
    capture(&store, "run1", &root, "a.txt", Some("then\n"));
    store.capture("pane", "run1", &root, &Path::new(&root).join("b.txt").to_string_lossy(), &FileEdit::unknown());
    assert_eq!(store.files("pane", "run1", &root), ["a.txt"]);
    assert_eq!(store.paths("pane", "run1", &root), ["a.txt", "b.txt"]);
    let elsewhere = base.path().join("elsewhere");
    std::fs::create_dir_all(&elsewhere).unwrap();
    assert!(store.files("pane", "run1", &elsewhere.to_string_lossy()).is_empty());
    assert!(revert::plan(&store, "pane", "run1", &elsewhere.to_string_lossy()).is_empty());
}

// ---- 바뀐 부분별 받기·버리기 (MC-30) ----------------------------------------------------------------------------------------------

fn numbered(count: usize, ending: &str) -> String {
    (1..=count).map(|n| format!("line {n}{ending}")).collect()
}

/// The fingerprints of the file's hunks as the review reads them now.
fn prints(store: &FileOriginalStore, run: &str, root: &str, path: &str) -> Vec<String> {
    let review = revert::hunk_review(store, "pane", run, root, path).expect("a kept file");
    review["hunks"].as_array().unwrap().iter().map(|h| h["fingerprint"].as_str().unwrap().to_string()).collect()
}

fn refusals(store: &FileOriginalStore, run: &str, root: &str, path: &str) -> Vec<Option<String>> {
    let review = revert::hunk_review(store, "pane", run, root, path).unwrap();
    review["hunks"].as_array().unwrap().iter().map(|h| h["refusal"].as_str().map(str::to_string)).collect()
}

fn discard(store: &FileOriginalStore, run: &str, root: &str, path: &str, fingerprint: &str) -> Outcome {
    revert::discard_hunk(store, "pane", run, root, path, fingerprint, None, &remove)
}

#[test]
fn one_hunk_of_three_goes_back_and_the_file_stays_crlf_with_its_mark() {
    let (_base, root, store) = setup("hunk-one");
    let original = numbered(40, "\r\n");
    let now = original.replace("line 5\r\n", "FIVE\r\n").replace("line 20\r\n", "line 20\r\nadded\r\n").replace("line 35\r\n", "");
    write(Path::new(&root), "a.cs", &[b"\xEF\xBB\xBF".as_slice(), now.as_bytes()].concat());
    assert!(capture(&store, "run1", &root, "a.cs", Some(&original)));
    let names = prints(&store, "run1", &root, "a.cs");
    assert_eq!(names.len(), 3);
    assert_eq!(discard(&store, "run1", &root, "a.cs", &names[1]), Outcome::Reverted);
    let expected = original.replace("line 5\r\n", "FIVE\r\n").replace("line 35\r\n", "");
    assert_eq!(read(&root, "a.cs").unwrap(), [b"\xEF\xBB\xBF".as_slice(), expected.as_bytes()].concat());
    assert_eq!(prints(&store, "run1", &root, "a.cs"), [names[0].clone(), names[2].clone()], "the others keep their names");
    // The same hunk again: it is no longer there.
    assert_eq!(discard(&store, "run1", &root, "a.cs", &names[1]), Outcome::Refused(Refusal::Conflict));
}

#[test]
fn a_cp949_file_without_a_last_line_break_stays_so() {
    let (_base, root, store) = setup("hunk-cp949");
    let original = format!("{}한글 끝", numbered(30, "\n"));
    let now = original.replace("line 2\n", "둘째 줄\n").replace("한글 끝", "한글 끝 바뀜");
    let (bytes, _, unmappable) = encoding_rs::EUC_KR.encode(&now);
    assert!(!unmappable);
    write(Path::new(&root), "k.txt", &bytes);
    assert!(capture(&store, "run1", &root, "k.txt", Some(&original)));
    let names = prints(&store, "run1", &root, "k.txt");
    assert_eq!(names.len(), 2);
    assert_eq!(discard(&store, "run1", &root, "k.txt", &names[1]), Outcome::Reverted);
    let expected = original.replace("line 2\n", "둘째 줄\n");
    assert_eq!(read(&root, "k.txt").unwrap(), encoding_rs::EUC_KR.encode(&expected).0.into_owned());
}

#[test]
fn a_hunk_changed_after_the_review_read_it_is_refused() {
    let (_base, root, store) = setup("hunk-conflict");
    let original = numbered(20, "\n");
    write(Path::new(&root), "a.txt", original.replace("line 10\n", "TEN\n").as_bytes());
    assert!(capture(&store, "run1", &root, "a.txt", Some(&original)));
    let review = revert::hunk_review(&store, "pane", "run1", &root, "a.txt").unwrap();
    let shown = mighty_files::diff::Hunk::from_json(&review["changes"]["hunks"][0]).unwrap();
    let name = review["hunks"][0]["fingerprint"].as_str().unwrap().to_string();
    // The person edits the hunk's line after the review was read.
    let edited = original.replace("line 10\n", "TEN!\n");
    write(Path::new(&root), "a.txt", edited.as_bytes());
    assert_eq!(
        revert::discard_hunk(&store, "pane", "run1", &root, "a.txt", &name, Some(&shown), &remove),
        Outcome::Refused(Refusal::Conflict)
    );
    assert_eq!(read(&root, "a.txt").unwrap(), edited.as_bytes(), "nothing written");
    // Lines added above it only move it: the hunk as shown is still found and put back.
    let moved = format!("new top\n{}", original.replace("line 10\n", "TEN\n"));
    write(Path::new(&root), "a.txt", moved.as_bytes());
    assert_eq!(revert::discard_hunk(&store, "pane", "run1", &root, "a.txt", "not-a-name", Some(&shown), &remove), Outcome::Reverted);
    assert_eq!(read(&root, "a.txt").unwrap(), format!("new top\n{original}").as_bytes());
}

#[test]
fn a_later_run_blocks_only_the_hunks_whose_lines_it_changed() {
    let (_base, root, store) = setup("hunk-later");
    let original = numbered(40, "\n");
    let after_one = original.replace("line 5\n", "FIVE\n").replace("line 30\n", "THIRTY\n");
    write(Path::new(&root), "a.txt", after_one.as_bytes());
    assert!(capture(&store, "run1", &root, "a.txt", Some(&original)));
    assert!(capture(&store, "run2", &root, "a.txt", Some(&after_one)));
    write(Path::new(&root), "a.txt", after_one.replace("line 31\n", "thirty-one\n").as_bytes());
    assert_eq!(refusals(&store, "run1", &root, "a.txt"), [None, Some("laterRun".to_string())]);
    let names = prints(&store, "run1", &root, "a.txt");
    assert_eq!(discard(&store, "run1", &root, "a.txt", &names[1]), Outcome::Refused(Refusal::LaterRun));
    assert_eq!(discard(&store, "run1", &root, "a.txt", &names[0]), Outcome::Reverted);
    assert_eq!(read(&root, "a.txt").unwrap(), original.replace("line 30\n", "THIRTY\n").replace("line 31\n", "thirty-one\n").as_bytes());
    // The next run kept no original of the file: what it changed is unknown, so the whole file is blocked.
    let c = original.replace("line 5\n", "FIVE\n").replace("line 30\n", "THIRTY\n");
    write(Path::new(&root), "c.txt", c.as_bytes());
    assert!(capture(&store, "run1", &root, "c.txt", Some(&original)));
    store.capture("pane", "run2", &root, &Path::new(&root).join("c.txt").to_string_lossy(), &FileEdit::unknown());
    assert_eq!(refusals(&store, "run1", &root, "c.txt"), [Some("laterRun".to_string()), Some("laterRun".to_string())]);
}

#[test]
fn every_hunk_put_back_restores_the_original_and_a_new_file_goes_to_the_trash() {
    let (_base, root, store) = setup("hunk-all");
    let original = numbered(40, "\r\n");
    let now = original.replace("line 3\r\n", "x\n").replace("line 33\r\n", "y\n");
    write(Path::new(&root), "a.txt", now.as_bytes());
    assert!(capture(&store, "run1", &root, "a.txt", Some(&original)));
    for name in prints(&store, "run1", &root, "a.txt") {
        assert_eq!(discard(&store, "run1", &root, "a.txt", &name), Outcome::Reverted);
    }
    assert_eq!(read(&root, "a.txt").unwrap(), original.as_bytes(), "byte for byte, as reverting the file");
    let item = &revert::plan(&store, "pane", "run1", &root)[0];
    assert_eq!((item.progress.handled, item.progress.total, item.reviewed()), (2, 2, true));

    write(Path::new(&root), "new.txt", b"one\ntwo\n");
    assert!(capture(&store, "run1", &root, "new.txt", None));
    let trashed = std::sync::Mutex::new(0);
    let names = prints(&store, "run1", &root, "new.txt");
    assert_eq!(names.len(), 1);
    let outcome = revert::discard_hunk(&store, "pane", "run1", &root, "new.txt", &names[0], None, &|path| {
        *trashed.lock().unwrap() += 1;
        remove(path)
    });
    assert_eq!(outcome, Outcome::Reverted);
    assert_eq!(*trashed.lock().unwrap(), 1);
    assert!(read(&root, "new.txt").is_none());
}

#[test]
fn taken_hunks_are_kept_across_a_restart_and_make_the_file_reviewed() {
    let (base, root, store) = setup("hunk-accept");
    let original = numbered(40, "\n");
    write(Path::new(&root), "a.txt", original.replace("line 5\n", "FIVE\n").replace("line 30\n", "THIRTY\n").as_bytes());
    write(Path::new(&root), "b.txt", b"b now\n");
    assert!(capture(&store, "run1", &root, "a.txt", Some(&original)));
    assert!(capture(&store, "run1", &root, "b.txt", Some("b then\n")));
    let names = prints(&store, "run1", &root, "a.txt");
    assert!(revert::accept_hunks(&store, "pane", "run1", &root, &["a.txt".to_string()], Some(&names[..1])));
    let progress = |store: &FileOriginalStore| -> Vec<(usize, usize, bool)> {
        revert::plan(store, "pane", "run1", &root).iter().map(|i| (i.progress.handled, i.progress.total, i.reviewed())).collect()
    };
    assert_eq!(progress(&store), [(1, 2, false), (0, 1, false)]);
    // The app starts again: a new store over the same folder reads the same decisions.
    let again = FileOriginalStore::new(base.path().join("originals"), changes::MAXIMUM_BYTES, common::handles());
    let review = revert::hunk_review(&again, "pane", "run1", &root, "a.txt").unwrap();
    assert_eq!(review["hunks"][0]["decision"], "accepted");
    assert!(review["hunks"][1].get("decision").is_none());
    assert_eq!((review["handled"].as_u64(), review["total"].as_u64()), (Some(1), Some(2)));
    // 남은 부분 모두 받기: every file's remaining hunks.
    assert!(revert::accept_hunks(&again, "pane", "run1", &root, &["a.txt".to_string(), "b.txt".to_string()], None));
    assert_eq!(progress(&again), [(2, 2, true), (1, 1, true)]);
}
