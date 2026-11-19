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
