//! What the style checks share: the frozen files read in place (the corpus, the fixtures, the goldens, the repository manifests),
//! temporary profiles, the test vocabulary and the golden's fixed run state (C# `StyleFixtures.cs`, `ClaudePlanStyleVerification.Fixture`).
#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::sync::Arc;

use mighty_core::locale::Locale;
use mighty_core::model::{BackgroundTask, BackgroundWork, TodoItem, TodoProgress};
use mighty_styles::manifest::Vocabulary;
use mighty_styles::{PlainAccess, RegisteredStyle, StyleRegistry, TrustAccess, decoder};

/// The repository's root.
pub fn repository() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../..")
}

pub fn styles_folder() -> PathBuf {
    repository().join("styles")
}

/// A file of `styles/` by its relative path (`conformance/valid/minimal.json`).
pub fn fixture(relative: &str) -> Vec<u8> {
    std::fs::read(styles_folder().join(relative)).unwrap_or_else(|e| panic!("styles/{relative}: {e}"))
}

pub fn fixture_text(relative: &str) -> String {
    String::from_utf8(fixture(relative)).expect("utf-8").replace("\r\n", "\n")
}

/// Every `*.json` of a folder of `styles/`, by name in ordinal order.
pub fn corpus(folder: &str) -> Vec<(String, Vec<u8>)> {
    let mut files: Vec<(String, Vec<u8>)> = std::fs::read_dir(styles_folder().join(folder))
        .expect("folder")
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.ends_with(".json"))
        .map(|n| {
            let bytes = std::fs::read(styles_folder().join(folder).join(&n)).expect("read");
            (n, bytes)
        })
        .collect();
    files.sort_by(|a, b| a.0.cmp(&b.0));
    files
}

/// A folder of its own under the temporary folder, removed when dropped (C# `Verification.Temp()`).
pub struct Temp(pub PathBuf);

impl Temp {
    pub fn new() -> Temp {
        let path = std::env::temp_dir().join(format!("mighty-styles-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&path).expect("temp");
        Temp(mighty_core::workspace_files::real_path(&path).unwrap_or(path))
    }
    pub fn join(&self, part: &str) -> PathBuf {
        self.0.join(part)
    }
}

impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

pub fn access() -> Arc<dyn TrustAccess> {
    Arc::new(PlainAccess)
}

pub fn korean() -> Locale {
    Locale::new("ko")
}

pub fn load(profile: &Path, workspace: &Path) -> StyleRegistry {
    StyleRegistry::load(profile, workspace, access(), korean())
}

/// The bundled styles, from an empty profile.
pub fn bundled(id: &str) -> RegisteredStyle {
    let root = Temp::new();
    load(&root.join("profile"), &root.join("workspace")).styles.into_iter().find(|s| s.id() == id).expect("bundled style")
}

/// `StyleFixtures.TestCapabilities`.
pub fn test_capabilities() -> Vocabulary {
    Vocabulary::from([("acme.thing".to_string(), vec!["absent".to_string(), "open".to_string(), "complete".to_string()])])
}

/// `StyleFixtures.Registered(bytes, path, capabilities)`: a user style, approved.
pub fn registered(bytes: &[u8], path: &str, capabilities: Option<&Vocabulary>) -> RegisteredStyle {
    let manifest = decoder::decode(bytes, "user", capabilities).unwrap_or_else(|e| panic!("{path}: {e}"));
    RegisteredStyle::decoded(manifest, "user", path, None, "approved", bytes)
}

/// `StyleFixtures.Style(id)`: `styles/fixtures/<id>.json` under the test vocabulary.
pub fn fixture_style(id: &str) -> RegisteredStyle {
    registered(&fixture(&format!("fixtures/{id}.json")), &format!("/tmp/{id}.json"), Some(&test_capabilities()))
}

/// `ClaudePlanStyleVerification.Fixture()`: the golden's fixed run state.
pub fn run_state_fixture() -> (TodoProgress, BackgroundWork) {
    let items = (1..=7)
        .map(|i| TodoItem {
            content: Some(format!("step {i}")),
            status: Some(
                if i <= 3 {
                    "completed"
                } else if i == 4 {
                    "in_progress"
                } else {
                    "pending"
                }
                .into(),
            ),
            active_form: Some(format!("doing step {i}")),
            id: None,
        })
        .collect();
    let task = |id: &str, kind: &str, description: &str, started: &str, status: &str, ended: Option<&str>| BackgroundTask {
        id: Some(id.into()),
        kind: Some(kind.into()),
        description: Some(description.into()),
        started_at: Some(started.into()),
        status: Some(status.into()),
        tool_use_id: None,
        summary: None,
        ended_at: ended.map(str::to_string),
    };
    let work = BackgroundWork {
        tasks: Some(vec![
            task("bg-1", "agent", "review the diff", "2026-10-06T01:00:00.000Z", "running", None),
            task("bg-2", "shell", "npm test", "2026-10-06T01:00:05.000Z", "completed", Some("2026-10-06T01:01:10.000Z")),
        ]),
        turn_ended: true,
    };
    (TodoProgress { items: Some(items), run_id: None }, work)
}

/// The decode's code, or None when accepted.
pub fn code(bytes: &[u8], source: &str, vocabulary: Option<&Vocabulary>) -> Option<String> {
    decoder::decode(bytes, source, vocabulary).err().map(|e| e.code)
}
