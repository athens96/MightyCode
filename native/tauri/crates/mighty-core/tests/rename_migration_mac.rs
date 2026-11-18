//! The Mac's half of RenameMigration (Swift `RenameMigration.swift`, Mac stage M-B): Swift's names (`MightyClaude Native`, the Claude
//! project folders by their marker, paths written with escaped slashes) and the old defaults domain copied once, over a fake of the
//! settings (the Mac's own over two UserDefaults domains is mighty-platform-macos `prefs.rs`, tested there in test domains).

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};

use mighty_core::rename_migration::{self, LegacySettings, MAC_NAMES, NoRegistry, Report};

struct Root(PathBuf);

impl Root {
    fn new(name: &str) -> Root {
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let root = std::env::temp_dir().join(format!("mighty-rename-mac-{name}-{}-{nanos}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        Root(root)
    }
}

impl Drop for Root {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Two settings domains in memory: the new one and the old bundle id's.
#[derive(Default)]
struct Domains {
    current: RefCell<BTreeMap<String, String>>,
    legacy: Option<BTreeMap<String, String>>,
}

impl LegacySettings for Domains {
    fn copied(&self) -> bool {
        self.current.borrow().contains_key(rename_migration::SETTINGS_MARKER)
    }
    fn legacy_keys(&self) -> Vec<String> {
        self.legacy.as_ref().map(|legacy| legacy.keys().cloned().collect()).unwrap_or_default()
    }
    fn has(&self, key: &str) -> bool {
        self.current.borrow().contains_key(key)
    }
    fn copy(&self, key: &str) -> io::Result<()> {
        let value = self.legacy.as_ref().and_then(|legacy| legacy.get(key)).cloned().ok_or_else(|| io::Error::other("none"))?;
        self.current.borrow_mut().insert(key.into(), value);
        Ok(())
    }
    fn mark(&self) -> io::Result<()> {
        self.current.borrow_mut().insert(rename_migration::SETTINGS_MARKER.into(), "true".into());
        Ok(())
    }
}

/// A record naming `path` plainly and, where it has slashes, with them escaped as `JSONEncoder` writes them.
fn record(path: &Path) -> String {
    let plain = serde_json::to_string(&path.to_string_lossy()).unwrap();
    let escaped = plain.replace('/', "\\/");
    format!(r#"{{"worktreePath":{plain},"other":{escaped}}}"#)
}

/// `RenameMigrationTests.swift:8 "theOldDataFolderWorktreesConversationsAndSettingsMoveOnce"`.
#[test]
fn the_old_data_folder_worktrees_conversations_and_settings_move_once() {
    let root = Root::new("once");
    let (support, home) = (root.0.join("Support"), root.0.join("home"));
    let old_data = support.join("MightyClaude Native");
    let old_worktree = home.join(".mightyclaude").join("worktrees").join("child-1");
    let projects = home.join(".claude").join("projects");
    let old_project = projects.join("-Users-me--mightyclaude-worktrees-child-1");
    for folder in [&old_data, &old_worktree, &old_project] {
        std::fs::create_dir_all(folder).unwrap();
    }
    std::fs::write(old_data.join("delegation-state.json"), record(&old_worktree)).unwrap();
    let settings = Domains {
        current: RefCell::new([("already".to_string(), "kept".to_string())].into()),
        legacy: Some([("theme".to_string(), "dark".to_string()), ("already".to_string(), "old".to_string())].into()),
    };

    let report = rename_migration::run_with(&MAC_NAMES, &support, &home, &home.join(".claude"), &NoRegistry, &settings);
    assert!(report.moved_data_folder && report.moved_worktrees && report.copied_settings == 1, "{report:?}");
    let data = support.join("MightyCode");
    let new_worktree = home.join(".mightycode").join("worktrees").join("child-1");
    assert!(!old_data.exists() && new_worktree.is_dir());
    assert_eq!(report.worktrees.iter().map(|w| w.file_name().unwrap().to_string_lossy().into_owned()).collect::<Vec<_>>(), ["child-1"]);
    assert_eq!(std::fs::read_to_string(data.join("delegation-state.json")).unwrap(), record(&new_worktree));
    assert!(projects.join("-Users-me--mightycode-worktrees-child-1").is_dir(), "the conversations follow the worktree");
    // A setting the new app already has is not overwritten.
    assert_eq!(settings.current.borrow().get("theme").map(String::as_str), Some("dark"));
    assert_eq!(settings.current.borrow().get("already").map(String::as_str), Some("kept"));

    // Once is enough: an old folder made again is left alone while the new one is there, and a setting cleared later stays cleared.
    std::fs::create_dir_all(&old_data).unwrap();
    settings.current.borrow_mut().remove("theme");
    let again = rename_migration::run_with(&MAC_NAMES, &support, &home, &home.join(".claude"), &NoRegistry, &settings);
    assert_eq!(again, Report::default());
    assert!(old_data.is_dir() && !settings.current.borrow().contains_key("theme"));
}

/// A record is rewritten atomically (nothing left beside it), and on the Mac one that is not UTF-8 is left as it is, as Swift's
/// `String(contentsOf:encoding: .utf8)` leaves it; Windows (C#) reads it with its bad bytes replaced.
#[test]
fn records_are_rewritten_atomically_and_the_mac_leaves_one_that_is_not_utf8() {
    let root = Root::new("records");
    let good = root.0.join("good.json");
    let bad = root.0.join("bad.json");
    std::fs::write(&good, r#"{"p":"/h/.mightyclaude/worktrees/a"}"#).unwrap();
    let mut bytes = br#"{"p":"/h/.mightyclaude/worktrees/a","x":""#.to_vec();
    bytes.extend_from_slice(b"\xff\"}");
    std::fs::write(&bad, &bytes).unwrap();
    rename_migration::rewrite_paths_as(&good, "/h/.mightyclaude", "/h/.mightycode", false);
    rename_migration::rewrite_paths_as(&bad, "/h/.mightyclaude", "/h/.mightycode", false);
    assert_eq!(std::fs::read_to_string(&good).unwrap(), r#"{"p":"/h/.mightycode/worktrees/a"}"#);
    assert_eq!(std::fs::read(&bad).unwrap(), bytes, "not UTF-8: left alone");
    rename_migration::rewrite_paths(&bad, "/h/.mightyclaude", "/h/.mightycode");
    assert!(String::from_utf8(std::fs::read(&bad).unwrap()).unwrap().contains("/h/.mightycode/worktrees/a"), "C#'s lossy read");
    let names: Vec<String> = std::fs::read_dir(&root.0).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
    assert_eq!(names.len(), 2, "no temporary file left: {names:?}");
}

/// `RenameMigrationTests.swift:43 "nothingFromBeforeTheRenameIsNothingToDo"`.
#[test]
fn nothing_from_before_the_rename_is_nothing_to_do() {
    let root = Root::new("nothing");
    let settings = Domains::default();
    let (support, home) = (root.0.join("Support"), root.0.join("home"));
    let report = rename_migration::run_with(&MAC_NAMES, &support, &home, &home.join(".claude"), &NoRegistry, &settings);
    assert_eq!(report, Report::default());
    assert!(!support.join("MightyCode").exists());
}
