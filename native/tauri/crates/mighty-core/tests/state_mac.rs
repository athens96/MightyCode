//! The Swift Mac app's `workspace-state.json` through Rust's load and save (mac-plan.md fact 16, M-B): the corpus `tools/parity-mac
//! state-corpus` writes with Swift's own `StateRepository` and `JSONEncoder` is loaded, saved and loaded again here; every key Swift
//! wrote must still be in what Rust saved, and `<case>.rust.json` is left beside each file for `parity-mac state-check`, which reads
//! both back with Swift's own decoder and wants every value Swift reads from its file to read the same from Rust's.
//!
//! The corpus folder is `$MIGHTY_PARITY_MAC_DIR/state` (the Mac CI job); without it the test says so and passes, unless
//! `MIGHTY_PARITY_MAC=require`.

use std::path::{Path, PathBuf};

use mighty_core::locale::Locale;
use mighty_core::store::Store;
use serde_json::Value;

fn corpus() -> Option<PathBuf> {
    match std::env::var_os("MIGHTY_PARITY_MAC_DIR") {
        Some(dir) => Some(PathBuf::from(dir).join("state")),
        None => {
            assert_ne!(
                std::env::var("MIGHTY_PARITY_MAC").as_deref(),
                Ok("require"),
                "MIGHTY_PARITY_MAC=require without MIGHTY_PARITY_MAC_DIR"
            );
            None
        }
    }
}

/// Every key path of `swift` missing from `rust`: objects by key, arrays of objects with an `id` by that id, other arrays by index.
fn missing(swift: &Value, rust: &Value, path: &str, out: &mut Vec<String>) {
    match (swift, rust) {
        (Value::Object(a), Value::Object(b)) => {
            for (key, value) in a {
                let here = format!("{path}.{key}");
                match b.get(key) {
                    Some(other) => missing(value, other, &here, out),
                    None => out.push(here),
                }
            }
        }
        (Value::Array(a), Value::Array(b)) => {
            for (index, value) in a.iter().enumerate() {
                let id = value.get("id").and_then(Value::as_str);
                let other = match id {
                    Some(id) => b.iter().find(|item| item.get("id").and_then(Value::as_str) == Some(id)),
                    None => b.get(index),
                };
                let here = match id {
                    Some(id) => format!("{path}[id={id}]"),
                    None => format!("{path}[{index}]"),
                };
                match other {
                    Some(other) => missing(value, other, &here, out),
                    None => out.push(here),
                }
            }
        }
        _ => {}
    }
}

/// Rust's load and save of one Swift-written file, in a profile of its own; the saved bytes.
fn round_trip(file: &Path, profile: &Path) -> Vec<u8> {
    std::fs::create_dir_all(profile).unwrap();
    std::fs::copy(file, profile.join("workspace-state.json")).unwrap();
    let mut store = Store::new(profile.to_path_buf(), None, true);
    let loaded = store.load(&Locale::new("en")).unwrap_or_else(|error| panic!("{} does not load: {error}", file.display()));
    store.save(&loaded).unwrap_or_else(|error| panic!("{} does not save: {error}", file.display()));
    std::fs::read(profile.join("workspace-state.json")).unwrap()
}

#[test]
fn the_swift_state_corpus_round_trips_with_no_field_lost() {
    let Some(dir) = corpus() else {
        eprintln!("no Swift state corpus (MIGHTY_PARITY_MAC_DIR): skipped; the Mac CI job runs it");
        return;
    };
    let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
        .unwrap_or_else(|error| panic!("{}: {error}", dir.display()))
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().is_some_and(|e| e == "json") && !path.to_string_lossy().ends_with(".rust.json"))
        .collect();
    files.sort();
    assert!(!files.is_empty(), "the corpus is empty");
    let work = std::env::temp_dir().join(format!("mighty-state-mac-{}", std::process::id()));
    let mut lost = Vec::new();
    for file in &files {
        let stem = file.file_stem().unwrap().to_string_lossy().into_owned();
        let saved = round_trip(file, &work.join(&stem));
        // Saving what Rust saved changes nothing: the round trip is stable.
        let rust_file = dir.join(format!("{stem}.rust.json"));
        std::fs::write(&rust_file, &saved).unwrap();
        let again = round_trip(&rust_file, &work.join(format!("{stem}-again")));
        assert_eq!(String::from_utf8_lossy(&again), String::from_utf8_lossy(&saved), "{stem}: a second load and save changed the bytes");
        let swift: Value = serde_json::from_slice(&std::fs::read(file).unwrap()).unwrap();
        let rust: Value = serde_json::from_slice(&saved).unwrap();
        let mut gone = Vec::new();
        missing(&swift, &rust, "", &mut gone);
        lost.extend(gone.into_iter().map(|path| format!("{stem}: {path}")));
    }
    let _ = std::fs::remove_dir_all(&work);
    assert!(lost.is_empty(), "keys Swift wrote that Rust's save dropped:\n{}", lost.join("\n"));
}
