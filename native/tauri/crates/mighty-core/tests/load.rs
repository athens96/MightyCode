//! C# `StateStore.LoadAsync` and `SaveAsync` on real folders (temporary ones): the damaged-file recovery, the rule that an
//! unreadable file stops every save, the legacy copy, the atomic write with its backup, and a store that may not write.

mod common;

use std::fs;
use std::path::Path;

use common::temp_dir;
use mighty_core::locale::Locale;
use mighty_core::store::{self, LoadSource, StateError, Store};

/// One workspace, in a folder that is absolute on this OS (`C:\a` is one only on Windows), so the load keeps it on every OS.
fn good() -> String {
    let root = if cfg!(windows) { r"C:\\a" } else { "/a" };
    format!(
        r#"{{"version":1,"workspaces":[{{"id":"w1","name":"A","path":"{root}","createdAt":"2026-10-24T00:00:00Z"}}],"sessions":[],"theme":"light"}}"#
    )
}
const OLDER: &str = r#"{"version":1,"workspaces":[],"sessions":[],"theme":"dark"}"#;

fn listing(dir: &Path) -> Vec<(String, Vec<u8>)> {
    let mut files: Vec<(String, Vec<u8>)> =
        fs::read_dir(dir).unwrap().flatten().map(|e| (e.file_name().to_string_lossy().into_owned(), fs::read(e.path()).unwrap())).collect();
    files.sort();
    files
}

fn en() -> Locale {
    Locale::new("en")
}

#[test]
fn a_zeroed_state_is_moved_aside_and_its_backup_restored() {
    let dir = temp_dir("zeroed");
    fs::write(dir.join(store::STATE_FILE), vec![0u8; good().len()]).unwrap();
    fs::write(dir.join("workspace-state.json.bak"), OLDER).unwrap();
    let mut store = Store::new(dir.clone(), None, true);
    let loaded = store.load(&Locale::new("ko")).unwrap();
    assert_eq!(store.source, Some(LoadSource::Backup));
    assert_eq!(loaded.theme(), "dark");
    let aside = store.aside.clone().unwrap();
    assert!(aside.starts_with("workspace-state.json.unreadable-"), "{aside}");
    assert!(dir.join(&aside).exists() && !dir.join(store::STATE_FILE).exists());
    assert_eq!(store.load_notice.as_deref(), Some(Locale::new("ko").get_with("store.notice.restoredBackup", &[("file", &aside)]).as_str()));
    // The next save writes a state again (the backup file stays the backup of nothing until then).
    store.save(&loaded).unwrap();
    assert!(dir.join(store::STATE_FILE).exists());
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn a_store_that_may_not_write_reports_what_it_would_do_and_changes_nothing() {
    let dir = temp_dir("readonly");
    fs::write(dir.join(store::STATE_FILE), vec![0u8; 64]).unwrap();
    fs::write(dir.join("workspace-state.json.bak"), OLDER).unwrap();
    let before = listing(&dir);
    let mut store = Store::new(dir.clone(), None, false);
    let loaded = store.load(&en()).unwrap();
    assert_eq!(store.source, Some(LoadSource::Backup));
    assert!(store.aside.is_some() && store.load_notice.is_some());
    assert!(matches!(store.save(&loaded), Err(StateError::WritesOff)));
    assert_eq!(listing(&dir), before, "a read-only store changed the folder");
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn a_damaged_state_without_a_readable_backup_starts_empty() {
    for backup in [None, Some(&b"{not json"[..]), Some(&[0u8; 8][..])] {
        let dir = temp_dir("damaged");
        let mut bytes = good().into_bytes();
        bytes[20] = 0;
        fs::write(dir.join(store::STATE_FILE), bytes).unwrap();
        if let Some(backup) = backup {
            fs::write(dir.join("workspace-state.json.bak"), backup).unwrap();
        }
        let mut store = Store::new(dir.clone(), None, true);
        let loaded = store.load(&en()).unwrap();
        assert_eq!(store.source, Some(LoadSource::DamagedEmpty));
        assert!(loaded.workspaces().is_empty());
        assert_eq!(loaded.antigravity_migrated, Some(true));
        let aside = store.aside.clone().unwrap();
        assert_eq!(store.load_notice.as_deref(), Some(en().get_with("store.notice.unreadableStartedEmpty", &[("file", &aside)]).as_str()));
        fs::remove_dir_all(dir).unwrap();
    }
}

#[test]
fn an_unreadable_state_stops_every_save() {
    let dir = temp_dir("unreadable");
    fs::write(dir.join(store::STATE_FILE), r#"{"version":2,"workspaces":[],"sessions":[]}"#).unwrap();
    fs::write(dir.join("workspace-state.json.bak"), OLDER).unwrap();
    let before = listing(&dir);
    let mut store = Store::new(dir.clone(), None, true);
    let error = store.load(&en()).unwrap_err();
    assert_eq!(error.kind(), "store.error.invalidFormat");
    assert!(!store.is_loaded());
    assert!(matches!(store.save(&mighty_core::AppSnapshot::default()), Err(StateError::NotLoaded)));
    assert_eq!(listing(&dir), before);
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn a_state_over_eight_megabytes_is_refused() {
    let dir = temp_dir("large");
    let padding = " ".repeat(8 * 1024 * 1024);
    fs::write(dir.join(store::STATE_FILE), format!("{{\"version\":1,\"workspaces\":[],\"sessions\":[]}}{padding}")).unwrap();
    let error = Store::new(dir.clone(), None, true).load(&en()).unwrap_err();
    assert_eq!(error.kind(), "store.error.fileTooLarge");
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn a_new_default_profile_takes_the_legacy_state() {
    let dir = temp_dir("new");
    let legacy = temp_dir("legacy");
    fs::write(legacy.join(store::STATE_FILE), good()).unwrap();
    // Read-only: read where it is, nothing copied.
    let mut reader = Store::new(dir.clone(), Some(legacy.clone()), false);
    let loaded = reader.load(&en()).unwrap();
    assert_eq!(reader.source, Some(LoadSource::Legacy(legacy.join(store::STATE_FILE))));
    assert_eq!(loaded.workspaces().len(), 1, "the legacy state's workspace is kept");
    assert!(!dir.join(store::STATE_FILE).exists(), "the legacy state was copied");
    // Writing: copied once, as C# copies it.
    let mut writer = Store::new(dir.clone(), Some(legacy.clone()), true);
    writer.load(&en()).unwrap();
    assert_eq!(fs::read(dir.join(store::STATE_FILE)).unwrap(), good().as_bytes());
    fs::remove_dir_all(dir).unwrap();
    fs::remove_dir_all(legacy).unwrap();
}

#[test]
fn a_save_keeps_the_file_it_replaces_as_the_backup_and_leaves_no_temporary_file() {
    let dir = temp_dir("save");
    let root = if cfg!(windows) { "C:\\\\a" } else { "/a" };
    let first = format!(
        r#"{{"version":1,"workspaces":[{{"id":"w1","name":"A","path":"{root}","createdAt":"2026-10-24T00:00:00Z"}}],"sessions":[]}}"#
    );
    fs::write(dir.join(store::STATE_FILE), &first).unwrap();
    let mut store = Store::new(dir.clone(), None, true);
    let mut loaded = store.load(&en()).unwrap();
    loaded.theme = Some("light".into());
    store.save(&loaded).unwrap();
    assert_eq!(fs::read_to_string(dir.join("workspace-state.json.bak")).unwrap(), first);
    let saved = String::from_utf8(fs::read(dir.join(store::STATE_FILE)).unwrap()).unwrap();
    assert!(saved.contains(r#""theme":"light""#));
    assert!(saved.contains(r#""sessionFiles":1"#), "the state is written split");
    // The one file of the state before the split is kept once, and the panes' folder is made beside the index.
    let mut names: Vec<String> = fs::read_dir(&dir).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
    names.sort();
    assert_eq!(names.len(), 4, "{names:?}");
    assert_eq!(&names[..3], ["sessions", "workspace-state.json", "workspace-state.json.bak"]);
    assert!(names[3].starts_with("workspace-state.json.before-session-files-"), "{names:?}");
    assert_eq!(fs::read_to_string(dir.join(&names[3])).unwrap(), first);
    // A workspace the load did not have, and no one approved, is refused.
    let mut foreign = store.snapshot.clone();
    foreign.workspaces_mut()[0].path = Some(if cfg!(windows) { "C:\\elsewhere".into() } else { "/elsewhere".into() });
    assert!(matches!(store.save(&foreign), Err(StateError::PathNotApproved)));
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn a_refused_replace_is_tried_again() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static CALLS: AtomicUsize = AtomicUsize::new(0);
    fn flaky(source: &Path, destination: &Path, backup: &Path) -> std::io::Result<()> {
        if CALLS.fetch_add(1, Ordering::SeqCst) < 3 {
            return Err(std::io::Error::new(std::io::ErrorKind::PermissionDenied, "held by a reader"));
        }
        store::portable_replace(source, destination, backup)
    }
    let dir = temp_dir("retry");
    let path = dir.join(store::STATE_FILE);
    fs::write(&path, "old").unwrap();
    store::atomic_write(&path, b"new", Some(&dir.join("workspace-state.json.bak")), flaky).unwrap();
    assert_eq!(CALLS.load(Ordering::SeqCst), 4);
    assert_eq!(fs::read(&path).unwrap(), b"new");
    assert_eq!(fs::read(dir.join("workspace-state.json.bak")).unwrap(), b"old");
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn the_saved_theme_is_read_before_the_load() {
    let dir = temp_dir("theme");
    assert_eq!(store::saved_theme(&dir), "dark");
    fs::write(dir.join(store::STATE_FILE), good()).unwrap();
    assert_eq!(store::saved_theme(&dir), "light");
    fs::remove_dir_all(dir).unwrap();
}

/// AntigravityVerification.cs:222 `ASavedGeminiCliPaneIsMovedOnce`, the load's part (C# `AntigravityMigration` in `StateStore.LoadAsync`):
/// a Gemini CLI pane's model and session are reset with a note, an agy model stays, other panes are untouched, the move is recorded and
/// runs once, and a new profile never held Gemini CLI panes. (`ModelUnavailable` is mighty-run's tests/antigravity.rs.)
#[test]
fn a_saved_gemini_cli_pane_is_moved_once() {
    let dir = temp_dir("agy-migration");
    let folder = serde_json::to_string(&dir.to_string_lossy()).unwrap();
    let saved = format!(
        r#"{{"version":1,"workspaces":[{{"id":"space","name":"Fixture","path":{folder}}}],"sessions":[
        {{"id":"legacy","workspaceId":"space","title":"Gemini","provider":"gemini","model":"gemini-2.5-pro","resumeId":"11111111-2222-4333-8444-555555555555"}},
        {{"id":"current","workspaceId":"space","title":"Antigravity","provider":"gemini","model":"gemini-3.8-flash-low"}},
        {{"id":"claude","workspaceId":"space","title":"Claude","model":"opus","resumeId":"claude-session"}}]}}"#
    );
    fs::write(dir.join(store::STATE_FILE), saved).unwrap();
    let mut state = Store::new(dir.clone(), None, true);
    let migrated = state.load(&en()).unwrap();
    assert_eq!(migrated.antigravity_migrated, Some(true), "the move is recorded");
    let pane = migrated.session("legacy").unwrap();
    assert!(
        pane.model() == "default"
            && pane.resume_id.is_none()
            && pane.logs().iter().map(|l| l.kind()).collect::<Vec<_>>() == ["system", "system"]
            && pane.logs()[0].text().contains("gemini-2.5-pro"),
        "a Gemini CLI model and session are reset with a note"
    );
    assert_eq!(migrated.session("current").unwrap().model(), "gemini-3.8-flash-low", "an agy model stays");
    let claude = migrated.session("claude").unwrap();
    assert!(claude.model() == "opus" && claude.resume_id.as_deref() == Some("claude-session"), "other panes are untouched");
    // Once: an agy conversation saved after the move is kept.
    let mut next = migrated.clone();
    for session in next.sessions_mut().iter_mut().filter(|s| s.id() == "legacy") {
        let mut changed = (**session).clone();
        changed.resume_id = Some("agy-conversation".into());
        changed.model = Some("gemini-4-argon-high".into());
        *session = std::sync::Arc::new(changed);
    }
    state.save(&next).unwrap();
    let again = Store::new(dir.clone(), None, true).load(&en()).unwrap();
    let pane = again.session("legacy").unwrap();
    assert!(
        again.antigravity_migrated == Some(true)
            && pane.resume_id.as_deref() == Some("agy-conversation")
            && pane.model() == "gemini-4-argon-high"
            && pane.logs().len() == 2,
        "the move runs once"
    );
    let fresh = dir.join("fresh");
    fs::create_dir_all(&fresh).unwrap();
    assert_eq!(
        Store::new(fresh, None, true).load(&en()).unwrap().antigravity_migrated,
        Some(true),
        "a new profile never held Gemini CLI panes"
    );
    fs::remove_dir_all(dir).unwrap();
}

/// P0-CORE-1: when `workspace-state.json` is missing but `workspace-state.json.bak` exists and is valid, `load()` recovers from
/// `.bak` instead of silently starting empty; a corrupt `.bak` still starts empty cleanly.
#[test]
fn a_missing_primary_state_recovers_from_its_valid_backup() {
    let dir = temp_dir("missing-primary-bak");
    fs::write(dir.join("workspace-state.json.bak"), good()).unwrap();
    // Read-only recovers without modifying disk.
    let before = listing(&dir);
    let mut reader = Store::new(dir.clone(), None, false);
    let loaded = reader.load(&en()).unwrap();
    assert_eq!(reader.source, Some(LoadSource::Backup));
    assert_eq!(loaded.theme(), "light");
    assert_eq!(loaded.workspaces().len(), 1);
    assert_eq!(listing(&dir), before, "read-only recovery leaves disk untouched");
    // Writable recovers and the next save restores the primary file.
    let mut writer = Store::new(dir.clone(), None, true);
    let loaded = writer.load(&en()).unwrap();
    assert_eq!(writer.source, Some(LoadSource::Backup));
    assert_eq!(loaded.workspaces().len(), 1);
    writer.save(&loaded).unwrap();
    assert!(dir.join(store::STATE_FILE).exists(), "save recreates the missing primary state file");
    fs::remove_dir_all(&dir).unwrap();

    // Corrupt `.bak` with missing primary falls back to empty without failing load.
    let corrupt = temp_dir("missing-primary-corrupt-bak");
    fs::write(corrupt.join("workspace-state.json.bak"), b"{not json").unwrap();
    let mut store = Store::new(corrupt.clone(), None, true);
    let loaded = store.load(&en()).unwrap();
    assert_eq!(store.source, Some(LoadSource::Empty));
    assert!(loaded.workspaces().is_empty());
    fs::remove_dir_all(&corrupt).unwrap();
}

/// P0-CORE-1: on Unix, `portable_replace` keeps `destination` intact when `source` cannot be renamed, refreshes `.bak` atomically
/// with `0o600` permissions, and `Store` enforces `0o700` on its directory and `0o600` on legacy and pre-split copies.
#[cfg(unix)]
#[test]
fn portable_replace_and_store_copies_are_crash_safe_and_owner_only() {
    use std::os::unix::fs::PermissionsExt;
    let mode = |path: &Path| fs::metadata(path).unwrap().permissions().mode() & 0o777;

    let dir = temp_dir("replace-unix");
    let source = dir.join("state.tmp");
    let destination = dir.join(store::STATE_FILE);
    let backup = dir.join("workspace-state.json.bak");
    fs::write(&destination, b"previous-state").unwrap();
    fs::set_permissions(&destination, fs::Permissions::from_mode(0o644)).unwrap();
    fs::write(&backup, b"stale-backup").unwrap();
    fs::set_permissions(&backup, fs::Permissions::from_mode(0o644)).unwrap();

    // If `source` does not exist, `destination` is NEVER removed or moved away.
    assert!(store::portable_replace(&dir.join("missing.tmp"), &destination, &backup).is_err());
    assert_eq!(fs::read(&destination).unwrap(), b"previous-state", "destination stayed intact on rename failure");
    assert_eq!(fs::read(&backup).unwrap(), b"previous-state", "backup holds previous destination");
    assert_eq!(mode(&backup), 0o600, "backup is written with 0o600 permissions");

    // Successful replace atomically swaps `source` -> `destination` and updates `.bak` with 0o600, leaving no temp files.
    fs::write(&source, b"next-state").unwrap();
    fs::set_permissions(&source, fs::Permissions::from_mode(0o600)).unwrap();
    store::portable_replace(&source, &destination, &backup).unwrap();
    assert_eq!(fs::read(&destination).unwrap(), b"next-state");
    assert_eq!(fs::read(&backup).unwrap(), b"previous-state");
    assert_eq!(mode(&backup), 0o600);
    assert!(!source.exists());
    let mut names: Vec<String> = fs::read_dir(&dir).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
    names.sort();
    assert_eq!(names, ["workspace-state.json", "workspace-state.json.bak"]);
    fs::remove_dir_all(&dir).unwrap();

    // Legacy migration (`copy_new`), store directory, and pre-split backup all get owner-only permissions.
    let legacy = temp_dir("legacy-perms");
    fs::write(legacy.join(store::STATE_FILE), good()).unwrap();
    fs::set_permissions(legacy.join(store::STATE_FILE), fs::Permissions::from_mode(0o644)).unwrap();
    let profile = temp_dir("profile-perms");
    fs::set_permissions(&profile, fs::Permissions::from_mode(0o755)).unwrap();
    let mut store = Store::new(profile.clone(), Some(legacy.clone()), true);
    let loaded = store.load(&en()).unwrap();
    assert_eq!(mode(&profile), 0o700, "store directory tightened to 0o700");
    assert_eq!(mode(&profile.join(store::STATE_FILE)), 0o600, "legacy copy created 0o600");
    store.save(&loaded).unwrap();
    let pre_split = fs::read_dir(&profile)
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .find(|p| p.file_name().unwrap().to_string_lossy().starts_with("workspace-state.json.before-session-files-"))
        .expect("pre-split backup");
    assert_eq!(mode(&pre_split), 0o600, "pre-split backup created 0o600");
    assert_eq!(mode(&profile.join("workspace-state.json.bak")), 0o600, "state backup created 0o600");
    fs::remove_dir_all(&profile).unwrap();
    fs::remove_dir_all(&legacy).unwrap();
}
