//! The repository's state-shaped fixtures load as C# loads them, and a save is a fixed point: what one save writes, loaded and
//! saved again, gives the same bytes (C#'s own guarantee that a restart does not change the file). Byte parity with C# itself is
//! `tests/parity.rs`.

mod common;

use std::path::PathBuf;

use common::{read_json, repo};
use mighty_core::locale::Locale;
use mighty_core::{demo, store};
use serde_json::{Value, json};

fn en() -> Locale {
    Locale::new("en")
}

/// Loads, saves, loads what it wrote and saves again: the two saves are the same bytes.
fn assert_fixed_point(name: &str, bytes: &[u8]) -> mighty_core::AppSnapshot {
    let loaded = store::read_state(bytes, &en()).unwrap_or_else(|e| panic!("{name}: {e}"));
    let (_, first) = store::encode_for_save(&loaded).unwrap();
    let again = store::read_state(&first, &en()).unwrap_or_else(|e| panic!("{name}: the saved state does not load: {e}"));
    let (_, second) = store::encode_for_save(&again).unwrap();
    assert_eq!(String::from_utf8_lossy(&first), String::from_utf8_lossy(&second), "{name}: a second save changed the file");
    loaded
}

#[test]
fn delegation_record_states_load_and_save_as_a_fixed_point() {
    let root = repo().join("scripts/fixtures/delegation-record");
    let mut states: Vec<PathBuf> = std::fs::read_dir(&root)
        .unwrap()
        .filter_map(|entry| entry.ok().map(|e| e.path().join("profile").join(store::STATE_FILE)))
        .filter(|path| path.exists())
        .collect();
    states.sort();
    assert!(states.len() >= 9, "expected the nine delegation-record profiles, found {}", states.len());
    for path in states {
        let text = std::fs::read_to_string(&path).unwrap();
        // Their "@{root}/…" paths are no folder: the load drops those workspaces and everything in them, as C# does.
        let raw = assert_fixed_point(&path.display().to_string(), text.as_bytes());
        assert!(raw.workspaces().is_empty() && raw.sessions().is_empty());
        let absolute = text.replace("@{root}", if cfg!(windows) { "C:/record" } else { "/record" });
        let loaded = assert_fixed_point(&path.display().to_string(), absolute.as_bytes());
        assert!(!loaded.workspaces().is_empty(), "{}", path.display());
    }
}

#[test]
fn unknown_fields_are_dropped_and_lenient_fields_load_as_nothing() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/unknown-fields.json");
    let text = std::fs::read_to_string(&path).unwrap();
    // The local workspace's folder must be absolute on this OS, as `C:\work\local` is only on Windows: elsewhere the load drops
    // that workspace, and with it the session holding the keyless graph step, before the step is normalized (C# as well).
    let local = r#""path": "C:\\work\\local""#;
    assert!(text.contains(local));
    let text = if cfg!(windows) { text } else { text.replace(local, r#""path": "/work/local""#) };
    // Its graph run's step has no key: C# throws reading `step.Key.Length` while it normalizes, and the load fails.
    assert_eq!(store::read_state(text.as_bytes(), &en()).unwrap_err().kind(), "crash");
    let text = text.replace(r#""steps": [{ "text": "one" }]"#, r#""steps": [{ "key": "k1", "content": "one" }]"#);
    let loaded = assert_fixed_point("unknown-fields.json", text.as_bytes());
    assert_eq!(loaded.workspaces().len(), 2, "both workspaces are kept, the local one and the remote one");
    assert_eq!(loaded.sessions().len(), 2, "both sessions are kept");
    let (_, saved) = store::encode_for_save(&loaded).unwrap();
    let written: Value = serde_json::from_slice(&saved).unwrap();
    // C# keeps no field it does not model (no JsonExtensionData in Core): a save drops them.
    let text = String::from_utf8_lossy(&saved);
    assert!(!text.contains("future"), "an unknown field was kept: {text}");
    // The lenient fields read their wrong-typed values as null.
    assert!(written.get("expandedWorkspaceIds").is_none());
    assert!(written.get("autoUpdateCLIs").is_none());
    let session = &written["sessions"][0];
    assert!(session.get("parentSessionId").is_none());
    assert!(session["settings"].get("advisor").is_none());
    // maxTurns and maxBudgetUsd are always written, null when unset, as C#'s RunSettingsJsonConverter writes them.
    assert_eq!(written["sessions"][1]["settings"]["maxTurns"], Value::Null);
    assert!(written["sessions"][1]["settings"].as_object().unwrap().contains_key("maxBudgetUsd"));
}

#[test]
fn the_demo_profile_loads() {
    let profile = read_json(&repo().join("native/contracts/fixtures/demo-profile.json"));
    for language in ["en", "ko"] {
        let root = if cfg!(windows) { PathBuf::from("C:\\demo\\Projects") } else { PathBuf::from("/demo/Projects") };
        let state = demo::demo_state(&profile, language, &root);
        let loaded = assert_fixed_point(&format!("demo ({language})"), &serde_json::to_vec(&state).unwrap());
        assert_eq!(loaded.workspaces().len(), 3);
        assert_eq!(loaded.sessions().len(), 12);
    }
}

#[test]
fn remote_references_load_as_the_contract_says() {
    // native/contracts/fixtures/remote-v2/state-references.json: which saved workspace shapes a load keeps.
    let cases = read_json(&repo().join("native/contracts/fixtures/remote-v2/state-references.json"));
    let cases = cases["cases"].as_array().unwrap();
    assert!(!cases.is_empty());
    for case in cases {
        let name = case["name"].as_str().unwrap();
        let workspace = &case["workspace"];
        assert_eq!(store::keeps_workspace(workspace), case["kept"].as_bool().unwrap(), "{name}");
        let state = json!({"version": 1, "workspaces": [workspace], "sessions": []});
        let loaded = store::read_state(&serde_json::to_vec(&state).unwrap(), &en()).unwrap();
        // A kept workspace survives normalize too when its path is absolute where it lives (the remote ones, and local ones on
        // their own OS).
        let survives = case["kept"].as_bool().unwrap()
            && (workspace.get("remote").is_some_and(|r| !r.is_null())
                || mighty_core::normalize::fully_qualified(workspace["path"].as_str().unwrap()));
        assert_eq!(loaded.workspaces().len() == 1, survives, "{name}: load");
    }
}

#[test]
fn every_state_shaped_contract_fixture_is_covered() {
    // The contract fixtures that hold a version 1 state (a top-level "version": 1 with "workspaces" and "sessions" arrays) must be
    // added to the parity corpus (examples/parity.rs `fixtures`): a new one fails this test until it is.
    let dir = repo().join("native/contracts/fixtures");
    let mut shaped = vec![];
    let mut stack = vec![dir.clone()];
    while let Some(folder) = stack.pop() {
        for entry in std::fs::read_dir(&folder).unwrap().flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|e| e == "json") {
                let Ok(value) = serde_json::from_slice::<Value>(&std::fs::read(&path).unwrap()) else { continue };
                if value["version"] == json!(1) && value["workspaces"].is_array() && value["sessions"].is_array() {
                    shaped.push(path.strip_prefix(&dir).unwrap().display().to_string());
                }
            }
        }
    }
    assert!(shaped.is_empty(), "state-shaped contract fixtures not in the parity corpus: {shaped:?}");
}

/// Several long-lived panes, each within its own 2 MiB of diagrams, together outgrow the 8 MiB file: the save still writes, cutting the
/// written copy of each pane's diagrams to its newest requests, and what it wrote loads again.
#[test]
fn a_state_too_large_for_the_file_is_saved_with_its_newest_requests() {
    let profile = read_json(&repo().join("native/contracts/fixtures/demo-profile.json"));
    let root = if cfg!(windows) { PathBuf::from("C:\\demo\\Projects") } else { PathBuf::from("/demo/Projects") };
    let mut state = demo::demo_state(&profile, "en", &root);
    let words = "x".repeat(30_000);
    let runs = |pane: usize| -> Value {
        Value::Array(
            (0..40)
                .map(|n| {
                    json!({
                        "id": format!("run-{pane}-{n:02}"),
                        "input": format!("request {n}"),
                        "status": "completed",
                        "provider": "claude",
                        "rootEntries": (0..2).map(|e| json!({"id": format!("e-{pane}-{n}-{e}"), "kind": "assistant", "text": words, "timestamp": "2026-11-20T00:00:00Z"})).collect::<Vec<_>>(),
                        "resultEntries": [],
                        "agents": [],
                    })
                })
                .collect(),
        )
    };
    let sessions = state["sessions"].as_array_mut().unwrap();
    let mut grown = 0;
    for (index, session) in sessions.iter_mut().enumerate() {
        if session["kind"] == "claude" && session["provider"] == "claude" {
            session["graphRuns"] = runs(index);
            grown += 1;
        }
    }
    assert!(grown >= 5, "enough panes to outgrow the file: {grown}");
    // Read past the file check (the state is built in memory, as the app holds it).
    let loaded = store::decode(&state).unwrap();
    let whole = mighty_core::json::Wire::to_json(&loaded).len();
    assert!(whole as u64 > store::MAXIMUM_STATE_BYTES, "the state itself is over the limit: {whole}");

    let (_, written) = store::encode_for_save(&loaded).expect("the save fits instead of failing");
    assert!(written.len() as u64 <= store::MAXIMUM_STATE_BYTES, "{} bytes", written.len());
    let again = store::read_state(&written, &en()).expect("what was written loads");
    for session in again.sessions().iter().filter(|s| s.graph_runs.as_ref().is_some_and(|r| !r.is_empty())) {
        let ids: Vec<&str> = session.graph_runs.as_ref().unwrap().iter().map(|r| r.id()).collect();
        assert!(ids.iter().any(|id| id.ends_with("-39")), "the newest request is kept: {ids:?}");
        assert!(!ids.iter().any(|id| id.ends_with("-00")), "the oldest is cut first: {ids:?}");
    }
    assert_eq!(again.sessions().len(), loaded.sessions().len(), "no pane is lost");
}
