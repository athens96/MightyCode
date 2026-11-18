//! The panel every style projects, against the goldens the macOS StyleGoldenContractTests record in `styles/golden` — compared whole,
//! byte for byte (C# `StyleGoldenVerification.cs`, L716/L717 by name). The goldens are read in place and never written.

mod common;

use std::collections::{BTreeMap, HashSet};

use common::*;
use mighty_styles::evaluator::StyleStateReading;
use mighty_styles::manifest::{items, text};
use mighty_styles::projection::{self, StylePanel};
use mighty_styles::state::{self, PLAN_STAGES};
use mighty_styles::{RegisteredStyle, StylePrerequisiteResult, decoder};
use serde_json::{Map, Value};

const CHECKLIST: &str = "- [x] one\n- [x] two\n- [x] three\n- [ ] four\n- [ ] five\n- [ ] six\n- [ ] seven\n";

fn events() -> Vec<(String, String)> {
    vec![("subagent.start".into(), "plan review".into()), ("tool.call".into(), "Read".into()), ("subagent.start".into(), "task 1".into())]
}

/// The `withState` reading (M/StyleGoldenSupport.swift stateReading).
fn state_reading(style: &RegisteredStyle, stage: &str) -> Option<StyleStateReading> {
    let sources = style.manifest.root.get("stateSources")?;
    let (todos, work) = run_state_fixture();
    let locale = korean();
    let files: Vec<_> = items(sources, "files")
        .iter()
        .map(|s| state::parse(CHECKLIST, text(s, "parser").unwrap_or_default(), text(s, "widget").unwrap_or_default(), &locale))
        .collect();
    Some(if style.evaluator().reads_plan_state() {
        state::reading(&style.manifest, &files, &events(), Some(stage), Some(&todos), Some(&work), &locale)
    } else {
        state::reading(&style.manifest, &files, &events(), None, None, None, &locale)
    })
}

fn projection_of(style: &RegisteredStyle) -> Map<String, Value> {
    let manifest = &style.manifest;
    let ready = StylePrerequisiteResult::ready();
    let probe = items(manifest.get("prerequisites"), "probes").first().cloned().unwrap_or(Value::Null);
    let missing = StylePrerequisiteResult {
        ready: false,
        missing: text(&probe, "missing").map(|l| vec![l.to_string()]).unwrap_or_default(),
        hint: text(&probe, "hint").map(str::to_string),
        install_command: None,
    };
    let none = BTreeMap::new();
    let make = |prompts: &[String],
                group: Option<&str>,
                prerequisites: &StylePrerequisiteResult,
                state: Option<&StyleStateReading>,
                running: bool|
     -> StylePanel { projection::make(style, prompts, group, &none, prerequisites, running, None, state) };
    let first = manifest.actions.first().map(|a| a.prompt("")).unwrap_or_default();
    let mut cases: Vec<(String, StylePanel)> = vec![
        ("empty".into(), make(&[], None, &ready, None, false)),
        ("afterFirstAction".into(), make(&[first], None, &ready, None, false)),
        ("notReady".into(), make(&[], None, &missing, None, false)),
        ("firstGroup".into(), make(&[], manifest.groups.first().map(|g| g.id.as_str()), &ready, None, false)),
        ("lastGroup".into(), make(&[], manifest.groups.last().map(|g| g.id.as_str()), &ready, None, false)),
    ];
    if let Some(reading) = state_reading(style, "executing") {
        cases.push(("withState".into(), make(&[], None, &ready, Some(&reading), false)));
    }
    // §1.17: one panel per plan stage for a style that reads it; a plan waiting for its answer is a running turn.
    if style.evaluator().reads_plan_state() {
        for stage in PLAN_STAGES {
            let reading = state_reading(style, stage)
                .unwrap_or_else(|| StyleStateReading { plan_stage: Some(stage.into()), ..StyleStateReading::default() });
            let name = format!("planStage{}{}", stage[..1].to_uppercase(), &stage[1..]);
            cases.push((name, make(&[], None, &ready, Some(&reading), stage == "awaitingApproval")));
        }
    }
    cases.into_iter().map(|(name, panel)| (name, panel.to_value())).collect()
}

fn goldens() -> BTreeMap<String, Vec<u8>> {
    corpus("golden").into_iter().collect()
}

fn compare(style: &RegisteredStyle, goldens: &BTreeMap<String, Vec<u8>>, compared: &mut HashSet<String>) {
    let name = format!("{}.panel.json", style.id());
    let expected = goldens.get(&name).unwrap_or_else(|| panic!("styles/golden has no {name}"));
    let produced = String::from_utf8(projection::canonical(&Value::Object(projection_of(style)))).expect("utf-8");
    // A Windows checkout may hold the golden with CRLF line ends; the bytes compared are the ones the Mac wrote.
    let golden = String::from_utf8(expected.clone()).expect("golden utf-8").replace("\r\n", "\n");
    if produced != golden {
        let ours: Vec<&str> = produced.split('\n').collect();
        let theirs: Vec<&str> = golden.split('\n').collect();
        let at = (0..ours.len().min(theirs.len())).find(|&i| ours[i] != theirs[i]).unwrap_or(ours.len().min(theirs.len()));
        let around = |lines: &[&str]| lines[at.saturating_sub(6)..(at + 4).min(lines.len())].join("\n");
        panic!(
            "{name} differs from the golden at line {} ({} lines against {}).\nproduced:\n{}\ngolden:\n{}",
            at + 1,
            ours.len(),
            theirs.len(),
            around(&ours),
            around(&theirs)
        );
    }
    compared.insert(name);
}

/// L716 "styles: every manifest, bundled style and test fixture projects its golden panel in full".
#[test]
fn styles_every_manifest_bundled_style_and_test_fixture_projects_its_golden_panel_in_full() {
    let goldens = goldens();
    let manifests: Vec<(String, Vec<u8>)> = corpus("");
    let mut compared = HashSet::new();
    assert!(!goldens.is_empty() && !manifests.is_empty(), "the goldens and the repository manifests are both in styles/");
    for (file, bytes) in &manifests {
        let manifest = decoder::decode(bytes, "user", None).unwrap_or_else(|e| panic!("{file}: {e}"));
        assert_eq!(*file, format!("{}.json", manifest.id()), "{file} must be named after its id");
        compare(&RegisteredStyle::decoded(manifest, "user", file, None, "approved", bytes), &goldens, &mut compared);
    }
    let root = Temp::new();
    let registry = load(&root.join("profile"), &root.join("workspace"));
    let bundled: Vec<&RegisteredStyle> = registry.styles.iter().filter(|s| s.source == "bundled").collect();
    let mut ids: Vec<&str> = bundled.iter().map(|s| s.id()).collect();
    ids.sort();
    assert_eq!(ids, ["claude-plan", "ouroboros"], "the bundled styles are Claude plan and Ouroboros");
    for style in &bundled {
        compare(style, &goldens, &mut compared);
    }
    let ouroboros = bundled.iter().find(|s| s.id() == "ouroboros").expect("ouroboros");
    assert!(
        state_reading(ouroboros, "executing").is_none()
            && !projection_of(ouroboros).keys().any(|k| k == "withState" || k.starts_with("planStage")),
        "a style without state sources has no withState and no plan-stage case"
    );
    let fixtures: Vec<String> = corpus("fixtures").into_iter().map(|(n, _)| n).collect();
    assert_eq!(fixtures, ["deck.json", "loop.json"], "styles/fixtures holds deck and loop");
    let mut projections = Vec::new();
    for file in &fixtures {
        let style = fixture_style(file.trim_end_matches(".json"));
        assert_eq!(*file, format!("{}.json", style.id()));
        projections.push(projection_of(&style));
        compare(&style, &goldens, &mut compared);
    }
    assert!(projections.iter().any(|p| p["firstGroup"] != p["lastGroup"]), "at least one golden's lastGroup differs from its firstGroup");
    let deck = projection_of(&fixture_style("deck"));
    assert_eq!(deck["firstGroup"]["next"], serde_json::json!(["sweep", "probe"]));
    assert_eq!(deck["lastGroup"]["next"], serde_json::json!(["draft", "cycle", "note", "reset", "peek"]));
    let peek = deck["empty"]["actions"].as_array().expect("actions").iter().find(|a| a["id"] == "peek").expect("peek").clone();
    assert_eq!(peek["flags"], serde_json::json!(["userInvoked", "readOnly"]), "an action's two flags are recorded in order");
    // §1.16: the loop fixture's file and run-event sources.
    let stateful = fixture_style("loop");
    let with_state = state_reading(&stateful, "executing").expect("the fixture declares state sources");
    assert!(
        with_state.widgets.len() == 2
            && with_state.widgets[0].kind == "progressBar"
            && with_state.widgets[0].value == 3
            && with_state.widgets[0].total == Some(7)
            && with_state.widgets[1].kind == "label"
            && with_state.widgets[1].text.as_deref() == Some("서브에이전트 2회 시작"),
        "a plan file reads as a bar and the sub-agents as a label: {:?}",
        with_state.widgets
    );
    assert_eq!(stateful.evaluator().current_phase::<&str>(&[], Some(&with_state.files), None).map(|p| p.id.as_str()), Some("build"));
    let recorded = projection_of(&stateful);
    assert!(recorded["withState"]["phase"]["id"] == "build" && recorded["withState"]["widgets"].as_array().map(Vec::len) == Some(2));
    assert!(recorded["empty"]["phase"]["id"] == "sketch" && recorded["empty"].get("widgets").is_none(), "the empty case reads nothing");
    assert!(!recorded.keys().any(|k| k.starts_with("planStage")));
    let left: Vec<&String> = goldens.keys().filter(|n| !compared.contains(*n)).collect();
    assert!(left.is_empty(), "goldens no style produced: {left:?}");
    assert_eq!(compared.len(), 6, "the six goldens of styles/golden are compared whole");
}

/// L717 "styles: the golden's serialisation rules are fixed".
#[test]
fn styles_the_goldens_serialisation_rules_are_fixed() {
    let style = bundled("ouroboros");
    let panel = projection::make::<&str>(&style, &[], None, &BTreeMap::new(), &StylePrerequisiteResult::ready(), false, None, None);
    let data = projection::serialise(&panel);
    let text = String::from_utf8(data.clone()).expect("utf-8");
    assert!(text.ends_with("}\n") && !text.ends_with("}\n\n"), "one trailing newline");
    assert!(!text.contains("\\/") && text.contains("add Q00/ouroboros") && !text.contains('\r'), "no escaped slash, no carriage return");
    assert!(text.starts_with("{\n  \"actions\" : ["), "keys sorted, two-space indent");
    assert_eq!(projection::serialise(&panel), data, "the same panel serialises to the same bytes");
    // What the phone is sent is the same panel through the wire's own serialiser: the same keys and values.
    let wire: Value = serde_json::from_str(&serde_json::to_string(&panel.to_value()).expect("wire")).expect("parse");
    assert_eq!(projection::canonical(&wire), data, "the wire payload is the golden's shape");
    let sample: Value = serde_json::from_str("{\"b\":[],\"a\":{},\"C\":\"x/\\\"y\\n\"}").expect("json");
    assert_eq!(
        String::from_utf8(projection::canonical(&sample)).expect("utf-8"),
        "{\n  \"a\" : {\n\n  },\n  \"b\" : [\n\n  ],\n  \"C\" : \"x/\\\"y\\n\"\n}\n",
        "empty arrays and objects, case-blind key order and escapes as the Mac writes them"
    );
}
