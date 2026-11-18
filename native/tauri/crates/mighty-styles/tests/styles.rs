//! C# `StylesVerification.cs`, ported by name (L712–L715, L718–L722, L730): the conformance corpus, reserved names, built-in
//! features, evaluation and trust, removal, the auto-allow judged at each request, which changes concern a style, the watcher and the
//! state boundaries. The frozen files are read in place.

mod common;

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::{Arc, Mutex};

use common::*;
use mighty_core::model::RunSession;
use mighty_core::permission::ToolPermissionRequest;
use mighty_run::model::StartRunRequest;
use mighty_styles::evaluator::StyleFileState;
use mighty_styles::permissions::{self, StyleAllowing};
use mighty_styles::{StylePrerequisiteResult, StyleRegistry, StyleTrustStore, decoder, files, manifest, projection, state, watcher};

fn trust(profile: &Path) -> StyleTrustStore {
    StyleTrustStore::new(&profile.join("style-trust"), access(), korean())
}

fn single(profile: &Path, workspace: &Path, id: &str) -> mighty_styles::RegisteredStyle {
    load(profile, workspace).styles.into_iter().find(|s| s.id() == id).unwrap_or_else(|| panic!("{id} is registered"))
}

/// L712 "styles Conformance".
#[test]
fn styles_conformance() {
    let mut failures = Vec::new();
    let valid = corpus("conformance/valid");
    let invalid = corpus("conformance/invalid");
    assert!(!valid.is_empty() && !invalid.is_empty());
    for (name, bytes) in &valid {
        match decoder::decode(bytes, "user", None) {
            Ok(m) if m.root.get("schema").and_then(|v| v.as_i64()) == Some(1) => {}
            Ok(_) => failures.push(format!("{name} is not schema 1")),
            Err(e) => failures.push(format!("{name} rejected {}", e.code)),
        }
    }
    for (name, bytes) in &invalid {
        let expected = name.trim_end_matches(".json");
        match decoder::decode(bytes, "user", None) {
            Ok(_) => failures.push(format!("{expected} was accepted")),
            Err(e) if e.code != expected => failures.push(format!("{expected} got {}", e.code)),
            Err(_) => {}
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("; "));
    let codes: std::collections::HashSet<&str> = invalid.iter().map(|(n, _)| n.trim_end_matches(".json")).collect();
    let reachable: std::collections::HashSet<&str> =
        decoder::CODES.iter().copied().filter(|c| !["E_ID_COLLISION", "E_TOO_LARGE", "E_CAPABILITY_MAP"].contains(c)).collect();
    assert!(
        reachable.difference(&codes).next().is_none(),
        "codes with no file in the corpus: {:?}",
        reachable.difference(&codes).collect::<Vec<_>>()
    );
    assert!(
        codes.difference(&reachable).next().is_none(),
        "corpus files named after no code: {:?}",
        codes.difference(&reachable).collect::<Vec<_>>()
    );
    assert!(invalid.len() == 51 && decoder::CODES.len() == 54, "51 of the 54 codes have a file");
    assert_eq!(code(&vec![0u8; decoder::MAXIMUM_BYTES + 1], "user", None).as_deref(), Some("E_TOO_LARGE"), "size precedes JSON");
    let minimal = fixture_text("conformance/valid/minimal.json");
    for glyph in ["☕", "👩‍💻", "⚙️", "🚀"] {
        let text = minimal
            .replace("\"takesText\":false", &format!("\"takesText\":false,\"glyph\":{}", serde_json::to_string(glyph).expect("glyph")));
        assert!(text != minimal && code(text.as_bytes(), "user", None).is_none(), "{glyph} is a glyph");
    }
}

/// L713 "styles: a style that is not bundled cannot take a bundled style's name".
#[test]
fn styles_a_style_that_is_not_bundled_cannot_take_a_bundled_styles_name() {
    let minimal = fixture_text("conformance/valid/minimal.json");
    let named = |name: &str, source: &str| {
        let text = minimal.replace("\"name\":\"코퍼스 최소\"", &format!("\"name\":{}", serde_json::to_string(name).expect("name")));
        code(text.as_bytes(), source, None)
    };
    assert_eq!(named("코퍼스 최소", "user"), None, "the fixture decodes as it is");
    assert_eq!(decoder::RESERVED_NAMES, ["Ouroboros", "클러드 플랜"]);
    let root = Temp::new();
    let mut names: Vec<String> = load(&root.join("profile"), &root.join("workspace"))
        .styles
        .iter()
        .filter(|s| s.source == "bundled")
        .map(|s| s.manifest.name().to_string())
        .collect();
    names.sort_by(|a, b| manifest::ordinal(a, b));
    let mut reserved: Vec<&str> = decoder::RESERVED_NAMES.to_vec();
    reserved.sort_by(|a, b| manifest::ordinal(a, b));
    assert_eq!(names, reserved, "the reserved names are the bundled styles' names");
    for name in ["Ouroboros", "ouroboros", "OURO BOROS", "ＯＵＲＯＢＯＲＯＳ", "클러드 플랜", "클러드플랜", "클러드\u{3000}플랜"]
    {
        assert_eq!(named(name, "user").as_deref(), Some("E_RESERVED_NAME"), "{name} passed the reserved-name check");
        assert_eq!(named(name, "bundled"), None, "{name} is refused for a bundled style");
    }
    assert!(
        named("Ouroboros Team", "user").is_none() && named("클러드 플랜 2", "user").is_none(),
        "a name that only contains a reserved one is free"
    );
}

/// L714 "styles: the app knows no built-in feature, and the rules that name one are refused in a fixed order".
#[test]
fn styles_the_app_knows_no_built_in_feature_and_the_rules_that_name_one_are_refused_in_a_fixed_order() {
    let minimal = fixture_text("conformance/valid/minimal.json");
    let with = |capabilities: Option<&str>, recommend: Option<&str>, initial: Option<&str>| {
        let mut text = minimal.clone();
        if let Some(c) = capabilities {
            text = text.replace("\"capabilities\":[]", &format!("\"capabilities\":{c}"));
        }
        if let Some(r) = recommend {
            text = text.replace("\"recommend\":{\"kind\":\"none\"}", &format!("\"recommend\":{r}"));
        }
        if let Some(i) = initial {
            text = text.replace("\"initialGroup\":{\"kind\":\"fixed\",\"group\":\"g\"}", &format!("\"initialGroup\":{i}"));
        }
        assert_ne!(text, minimal, "the fixture changed");
        text
    };
    let test = test_capabilities();
    assert!(manifest::app_vocabulary().is_empty(), "the app knows no built-in feature");
    for name in ["acme.thing", "a"] {
        let text = with(Some(&format!("[\"{name}\"]")), None, None);
        assert_eq!(code(text.as_bytes(), "user", None).as_deref(), Some("E_UNKNOWN_CAPABILITY"));
        assert_eq!(code(text.as_bytes(), "bundled", None).as_deref(), Some("E_UNKNOWN_CAPABILITY"));
    }
    assert!(code(minimal.as_bytes(), "user", None).is_none());
    let root = Temp::new();
    for style in load(&root.join("profile"), &root.join("workspace")).styles.iter().filter(|s| s.source == "bundled") {
        assert!(manifest::strings(&style.manifest.root, "capabilities").is_empty(), "{} declares no feature", style.id());
    }
    let recommend =
        "{\"kind\":\"capability\",\"capability\":\"acme.thing\",\"map\":{\"absent\":\"go\",\"open\":\"go\",\"complete\":\"go\"}}";
    let initial =
        "{\"kind\":\"capabilityState\",\"capability\":\"acme.thing\",\"map\":{\"absent\":\"g\",\"open\":\"g\",\"complete\":\"g\"}}";
    for (rule, group) in [(Some(recommend), None), (None, Some(initial))] {
        let undeclared = with(None, rule, group);
        let declared = with(Some("[\"acme.thing\"]"), rule, group);
        assert_eq!(code(undeclared.as_bytes(), "user", None).as_deref(), Some("E_CAPABILITY_UNDECLARED"));
        assert_eq!(code(undeclared.as_bytes(), "user", Some(&test)).as_deref(), Some("E_CAPABILITY_UNDECLARED"));
        assert_eq!(code(declared.as_bytes(), "user", None).as_deref(), Some("E_UNKNOWN_CAPABILITY"));
        assert_eq!(code(declared.as_bytes(), "user", Some(&test)), None);
    }
    let partial = with(
        Some("[\"acme.thing\"]"),
        Some("{\"kind\":\"capability\",\"capability\":\"acme.thing\",\"map\":{\"absent\":\"go\",\"open\":\"go\"}}"),
        None,
    );
    let extra = with(
        Some("[\"acme.thing\"]"),
        Some(
            "{\"kind\":\"capability\",\"capability\":\"acme.thing\",\"map\":{\"absent\":\"go\",\"open\":\"go\",\"complete\":\"go\",\"gone\":\"go\"}}",
        ),
        None,
    );
    let no_map = with(Some("[\"acme.thing\"]"), Some("{\"kind\":\"capability\",\"capability\":\"acme.thing\"}"), None);
    assert_eq!(code(partial.as_bytes(), "user", Some(&test)).as_deref(), Some("E_CAPABILITY_MAP"));
    assert_eq!(code(partial.as_bytes(), "user", None).as_deref(), Some("E_UNKNOWN_CAPABILITY"));
    assert_eq!(code(extra.as_bytes(), "user", Some(&test)).as_deref(), Some("E_UNKNOWN_REFERENCE"));
    assert_eq!(code(no_map.as_bytes(), "user", Some(&test)).as_deref(), Some("E_MISSING_FIELD"));
    let catalogue = fixture_style("deck");
    let evaluator = catalogue.evaluator();
    let state = |s: &str| BTreeMap::from([("acme.thing".to_string(), s.to_string())]);
    assert!(evaluator.recommended_action(&state("complete")) == Some("reset") && evaluator.recommended_action(&BTreeMap::new()).is_none());
    assert!(
        evaluator.initial_group(&state("open")).map(|g| g.id.as_str()) == Some("far")
            && evaluator.initial_group(&BTreeMap::new()).map(|g| g.id.as_str()) == Some("near")
    );
    let panel = projection::make::<&str>(&catalogue, &[], None, &state("complete"), &StylePrerequisiteResult::ready(), false, None, None);
    assert!(panel.recommended.as_deref() == Some("reset") && panel.groups.iter().filter(|g| g.selected).map(|g| g.id.as_str()).eq(["far"]));
    let wire: serde_json::Value = serde_json::from_slice(&projection::serialise(&panel)).expect("panel");
    assert_eq!(wire["attachments"], serde_json::json!([]), "attachments travel, always empty");
}

/// L715 "styles EvaluationAndTrust".
#[test]
fn styles_evaluation_and_trust() {
    let root = Temp::new();
    let workspace = root.join("workspace");
    let profile = root.join("profile");
    std::fs::create_dir_all(&workspace).expect("workspace");
    std::fs::create_dir_all(profile.join("styles")).expect("styles");
    let registry = load(&profile, &workspace);
    assert!(
        registry.styles.len() == 2 && registry.styles.iter().all(|s| s.runnable()),
        "both bundled styles loaded: {:?}",
        registry.rejections
    );
    let source = fixture("conformance/valid/with-state-sources.json");
    let file = profile.join("styles").join("custom.json");
    std::fs::write(&file, &source).expect("write");
    let style = single(&profile, &workspace, "corpus-state");
    assert!(!style.runnable(), "unapproved doesn't run");
    let store = trust(&profile);
    store.decide(&style, "approved").expect("approve");
    let registry = load(&profile, &workspace);
    assert!(registry.runnable(Some(style.id()), Some(&style.hash)).is_some(), "approval binds bytes and place");
    assert!(registry.runnable(Some(style.id()), Some("wrong")).is_none(), "pane hash also gates");
    let mut changed_bytes = source.clone();
    changed_bytes.push(b' ');
    std::fs::write(&file, &changed_bytes).expect("write");
    assert!(!StyleRegistry::unchanged(&style), "changed file invalidates preview before approve/run");
    let changed = single(&profile, &workspace, style.id());
    assert!(!changed.runnable(), "one changed byte asks again");
    store.decide(&changed, "revoked").expect("revoke");
    std::fs::write(&file, &source).expect("write");
    assert_eq!(single(&profile, &workspace, style.id()).approval, "revoked", "revocation survives old bytes returning");
    store.decide(&style, "unblock").expect("unblock");
    assert!(load(&profile, &workspace).runnable(Some(style.id()), Some(&style.hash)).is_none(), "unblock requires fresh approval");
    let evaluator = style.evaluator();
    assert_eq!(evaluator.current_phase::<&str>(&[], None, None).map(|p| p.id.as_str()), Some("draft"), "initial phase");
    let exists = BTreeMap::from([(0, StyleFileState { exists: true, all_checked: false })]);
    let done = BTreeMap::from([(0, StyleFileState { exists: true, all_checked: true })]);
    assert_eq!(evaluator.current_phase(&["/go"], Some(&exists), None).map(|p| p.id.as_str()), Some("build"), "state advances phase");
    assert_eq!(
        evaluator.current_phase(&["/go"], Some(&done), None).map(|p| p.id.as_str()),
        Some("done"),
        "completed checklist advances phase"
    );
    assert!(!evaluator.auto_allowed("AskUserQuestion"), "question never auto allowed");
    let registry = load(&profile, &workspace);
    let ouroboros = registry.styles.iter().find(|s| s.id() == "ouroboros").expect("ouroboros");
    let ouro = ouroboros.evaluator();
    let initial = ouro.current_phase::<&str>(&[], None, None);
    assert!(ouro.rewrite_action("한글 요청", initial, false, false, false, false).is_some(), "first bare draft rewrites");
    assert!(ouro.rewrite_action("한글 요청", initial, false, false, true, false).is_none(), "later request cannot rewrite");
    assert!(ouro.rewrite_action("/help", initial, false, false, false, false).is_none(), "slash never rewrite");
    let pane = RunSession {
        id: Some("style-pane".into()),
        workspace_id: Some("style-workspace".into()),
        provider: Some("claude".into()),
        agent_view_mode: Some("mighty".into()),
        mighty_style: Some("ouroboros".into()),
        ..RunSession::default()
    };
    let request = StartRunRequest::new("style-pane", "style-workspace", "claude", "test");
    let bound = permissions::bind(request, &pane, &profile, &workspace, access(), korean()).expect("bind");
    assert!(
        bound.style_auto_allow.as_ref().is_some_and(|g| !g.is_empty() && g.iter().all(|w| permissions::valid_wire_name(w))),
        "approved local style grants exact tools"
    );
    assert!(
        mighty_run::providers::arguments(&bound, "/plugin", None).expect("args").iter().any(|a| a == "--allowedTools"),
        "grants reach Claude argv"
    );
    for invalid in ["Bash", "mcp__plugin_x__*", "mcp__plugin_x__AskUserQuestion", "mcp__plugin_x__evil__tool"] {
        assert!(!permissions::valid_wire_name(invalid), "unsafe grant rejected {invalid}");
    }
}

/// A user style whose one tool is its own: the minimal corpus manifest with a plugin probe and an auto-allow entry.
fn auto_allow_fixture() -> Vec<u8> {
    fixture_text("conformance/valid/minimal.json")
        .replace("\"probes\":[]", "\"probes\":[{\"kind\":\"plugin\",\"prefix\":\"demo@\",\"missing\":\"demo\"}]")
        .replace("\"autoAllow\":[]", "\"autoAllow\":[{\"server\":\"plugin_demo_srv\",\"tool\":\"go\"}]")
        .into_bytes()
}

const AUTO_ALLOW_TOOL: &str = "mcp__plugin_demo_srv__go";

/// L718 "styles: removing a user style forgets the decision made for it".
#[test]
fn styles_removing_a_user_style_forgets_the_decision_made_for_it() {
    let root = Temp::new();
    let workspace = root.join("workspace");
    let profile = root.join("profile");
    std::fs::create_dir_all(&workspace).expect("workspace");
    std::fs::create_dir_all(profile.join("styles")).expect("styles");
    let source = fixture("conformance/valid/with-state-sources.json");
    let file = profile.join("styles").join("corpus-state.json");
    let store = trust(&profile);
    let registered = || single(&profile, &workspace, "corpus-state");
    std::fs::write(&file, &source).expect("write");
    let style = registered();
    store.decide(&style, "approved").expect("approve");
    assert!(registered().runnable());
    store.decide(&style, "revoked").expect("revoke");
    std::fs::remove_file(&file).expect("remove");
    std::fs::write(&file, &source).expect("write");
    assert_eq!(registered().approval, "revoked", "a refusal left behind meets the same id registered again");
    assert!(store.decide(&registered(), "approved").is_err(), "a refused place was approved");
    store.forget(&registered()).expect("forget");
    std::fs::remove_file(&file).expect("remove");
    assert!(store.load().expect("load").is_empty(), "nothing is kept for a removed style");
    std::fs::write(&file, &source).expect("write");
    assert_eq!(registered().approval, "pending", "registered again, it is asked about afresh");
    store.decide(&registered(), "approved").expect("approve");
    assert!(load(&profile, &workspace).runnable(Some("corpus-state"), Some(&style.hash)).is_some(), "and can be approved");
    store.forget(&registered()).expect("forget");
    std::fs::remove_file(&file).expect("remove");
    std::fs::write(&file, &source).expect("write");
    assert_eq!(registered().approval, "pending", "an approval does not outlive its file's removal");
    std::fs::write(profile.join("styles").join("corpus-min.json"), fixture("conformance/valid/minimal.json")).expect("write");
    store.decide(&single(&profile, &workspace, "corpus-min"), "approved").expect("approve");
    store.forget(&registered()).expect("forget");
    assert!(
        single(&profile, &workspace, "corpus-min").runnable() && store.load().expect("load").len() == 1,
        "only the removed style's decision goes"
    );
}

/// L719 "styles: a style's own tools are allowed request by request, never past a revocation".
#[test]
fn styles_a_styles_own_tools_are_allowed_request_by_request_never_past_a_revocation() {
    let root = Temp::new();
    let workspace = root.join("workspace");
    let profile = root.join("profile");
    std::fs::create_dir_all(&workspace).expect("workspace");
    std::fs::create_dir_all(profile.join("styles")).expect("styles");
    let bytes = auto_allow_fixture();
    let file = profile.join("styles").join("corpus-min.json");
    std::fs::write(&file, &bytes).expect("write");
    let store = trust(&profile);
    let style = single(&profile, &workspace, "corpus-min");
    let pane = RunSession {
        id: Some("style-pane".into()),
        workspace_id: Some("style-workspace".into()),
        provider: Some("claude".into()),
        agent_view_mode: Some("mighty".into()),
        mighty_style: Some(style.id().into()),
        mighty_style_hash: Some(style.hash.clone()),
        ..RunSession::default()
    };
    let request = ToolPermissionRequest::new("perm-1", "style-pane", "toolu_1", AUTO_ALLOW_TOOL, "{}", "go");
    let allows = |asked: &ToolPermissionRequest, from: Option<&RunSession>| {
        permissions::auto_allows(asked, from, &profile, &workspace, access(), korean())
    };
    let named = |tool: &str| ToolPermissionRequest { tool_name: tool.into(), ..request.clone() };
    let pane_with = |f: &dyn Fn(&mut RunSession)| {
        let mut p = pane.clone();
        f(&mut p);
        p
    };
    assert!(!allows(&request, Some(&pane)), "a style that was not approved allows nothing");
    store.decide(&style, "approved").expect("approve");
    assert!(allows(&request, Some(&pane)), "the approved style allows its own tool");
    assert!(
        !allows(&named("mcp__plugin_demo_srv__other"), Some(&pane))
            && !allows(&named("Bash"), Some(&pane))
            && !allows(&named("AskUserQuestion"), Some(&pane))
            && !allows(&named("mcp__plugin_demo_srv__AskUserQuestion"), Some(&pane)),
        "only the exact listed tool, never a question"
    );
    assert!(
        !allows(&ToolPermissionRequest { can_allow: false, ..request.clone() }, Some(&pane))
            && !allows(&ToolPermissionRequest { state: "allowed".into(), ..request.clone() }, Some(&pane)),
        "only a pending request that can be allowed"
    );
    assert!(
        !allows(&request, Some(&pane_with(&|p| p.agent_view_mode = Some("default".into()))))
            && !allows(&request, Some(&pane_with(&|p| p.mighty_style = None)))
            && !allows(
                &request,
                Some(&pane_with(&|p| {
                    p.mighty_style = Some("ouroboros".into());
                    p.mighty_style_hash = None;
                }))
            )
            && !allows(&request, Some(&pane_with(&|p| p.mighty_style_hash = Some("other".into()))))
            && !allows(&request, Some(&pane_with(&|p| p.provider = Some("codex".into()))))
            && !allows(&request, None),
        "only for the pane's own style, as the pane chose it"
    );
    let mut changed = bytes.clone();
    changed.push(b' ');
    std::fs::write(&file, &changed).expect("write");
    assert!(!allows(&request, Some(&pane)), "changed bytes stop it");
    std::fs::write(&file, &bytes).expect("write");
    assert!(allows(&request, Some(&pane)), "the approved bytes back, it allows again");
    store.decide(&style, "revoked").expect("revoke");
    assert!(!allows(&request, Some(&pane)), "a revocation stops it at once");
    store.decide(&style, "unblock").expect("unblock");
    assert!(!allows(&request, Some(&pane)), "unblocking alone does not allow");
    store.decide(&style, "approved").expect("approve");
    assert!(allows(&request, Some(&pane)), "a fresh approval does");
    std::fs::remove_file(&file).expect("remove");
    assert!(!allows(&request, Some(&pane)), "a removed style allows nothing");
    std::fs::write(&file, &bytes).expect("write");
    let bound = permissions::bind(
        StartRunRequest::new("style-pane", "style-workspace", "claude", "test"),
        &pane,
        &profile,
        &workspace,
        access(),
        korean(),
    )
    .expect("bind");
    let listed = mighty_run::providers::arguments(&bound, "/plugin", None).expect("args");
    let at = listed.iter().position(|a| a == "--allowedTools").expect("listed");
    let expected: Vec<&str> = std::iter::once(AUTO_ALLOW_TOOL).chain(mighty_run::flow_steps::TASK_TOOLS).collect();
    assert!(
        listed[at + 1] == expected.join(",") && listed.iter().filter(|a| *a == "--allowedTools").count() == 1,
        "a host without the card launches with the list"
    );
    let hosted = mighty_run::permissions::ProviderInput::prepare(&bound, "/plugin", true).expect("prepare").arguments;
    let at = hosted.iter().position(|a| a == "--allowedTools").expect("task tools");
    let prompts = hosted.iter().position(|a| a == "--permission-prompts").expect("host prompts");
    assert!(
        hosted.iter().filter(|a| *a == "--allowedTools").count() == 1
            && hosted[at + 1] == mighty_run::flow_steps::TASK_TOOLS.join(",")
            && !hosted.iter().any(|a| a == AUTO_ALLOW_TOOL)
            && hosted[prompts + 1] == "host",
        "a host with the card launches without the style's tools, the task tools still named"
    );
    assert!(
        mighty_run::permissions::ProviderInput::prepare(&bound, "/plugin", false)
            .expect("prepare")
            .arguments
            .iter()
            .any(|a| a == "--allowedTools"),
        "prompts off keeps the list"
    );
}

/// L720 "styles: a run allows its style's tools until the style is revoked": the service's half (C# `DesktopService.ReceiveToolPermission`)
/// over the real registry and trust store — the approved style's request is answered allowed without reaching the pane and its settling
/// is no news; once the style is revoked the next request of the same kind waits for the user. (C# drives it through a real child
/// process; here the answer is recorded where the service would send it.)
#[test]
fn styles_a_run_allows_its_styles_tools_until_the_style_is_revoked() {
    let root = Temp::new();
    let workspace = root.join("workspace");
    let profile = root.join("profile");
    std::fs::create_dir_all(&workspace).expect("workspace");
    std::fs::create_dir_all(profile.join("styles")).expect("styles");
    std::fs::write(profile.join("styles").join("corpus-min.json"), auto_allow_fixture()).expect("write");
    let store = trust(&profile);
    let style = single(&profile, &workspace, "corpus-min");
    store.decide(&style, "approved").expect("approve");
    let pane = RunSession {
        id: Some("pane".into()),
        workspace_id: Some("ws".into()),
        provider: Some("claude".into()),
        agent_view_mode: Some("mighty".into()),
        mighty_style: Some(style.id().into()),
        mighty_style_hash: Some(style.hash.clone()),
        ..RunSession::default()
    };
    let gate = StyleAllowing::default();
    let answered = Arc::new(Mutex::new(Vec::<String>::new()));
    let shown = std::cell::RefCell::new(Vec::<String>::new());
    let receive = |request: ToolPermissionRequest| {
        let record = answered.clone();
        let id = request.id.clone();
        if gate.receive(
            &request,
            || permissions::auto_allows(&request, Some(&pane), &profile, &workspace, access(), korean()),
            move || {
                record.lock().expect("answers").push(id);
                true
            },
        ) {
            shown.borrow_mut().push(request.state.clone());
        }
    };
    let pending = ToolPermissionRequest::new("style-1", "pane", "tool-style-1", AUTO_ALLOW_TOOL, "{\"step\":1}", "go");
    receive(pending.clone());
    receive(ToolPermissionRequest { state: "allowed".into(), ..pending.clone() });
    assert_eq!(*answered.lock().expect("answers"), ["style-1"], "the style's own tool is allowed once");
    assert!(shown.borrow().is_empty(), "a request the style allows never reaches the pane: {:?}", shown.borrow());
    store.decide(&style, "revoked").expect("revoke");
    let next = ToolPermissionRequest::new("style-2", "pane", "tool-style-2", AUTO_ALLOW_TOOL, "{\"step\":1}", "go");
    receive(next.clone());
    receive(ToolPermissionRequest { state: "denied".into(), ..next });
    assert_eq!(answered.lock().expect("answers").len(), 1, "after the revocation the same tool is not allowed by the style");
    assert_eq!(*shown.borrow(), ["pending", "denied"], "the request waited for the user and settled as answered");
}

/// L721 "styles: which file changes make a pane read its style again".
#[test]
fn styles_which_file_changes_make_a_pane_read_its_style_again() {
    let whole = decoder::decode(&fixture("conformance/valid/with-state-sources.json"), "user", None).expect("whole");
    let plain = decoder::decode(&fixture("conformance/valid/minimal.json"), "user", None).expect("plain");
    let root_sources = ",{\"path\":\"status.json\",\"parser\":\"json\",\"widget\":\"list\"},{\"path\":\"NOTES.md\",\"parser\":\"markdownChecklist\",\"widget\":\"label\"}";
    let text = fixture_text("conformance/valid/with-state-sources.json");
    assert!(text.contains(root_sources), "the fixture's root-level file sources");
    let manifest = decoder::decode(text.replace(root_sources, "").as_bytes(), "user", None).expect("manifest");
    let workspace = std::env::temp_dir().join("mighty-watch-workspace");
    let at = |parts: &[&str]| parts.iter().fold(workspace.clone(), |p, part| p.join(part));
    assert!(state::affects(&whole, &workspace, &at(&["src", "a.cs"])) && state::affects(&whole, &workspace, &at(&["NOTES.md"])));
    assert!(
        state::affects(&manifest, &workspace, &at(&["docs", "plans", "plan.md"]))
            && state::affects(&manifest, &workspace, &at(&["docs", "plans"]))
            && state::affects(&manifest, &workspace, &at(&["docs", "plans", "deep", "x.md"]))
    );
    assert!(state::affects(&manifest, &workspace, &at(&["docs"])), "a parent of the folder appearing");
    assert!(
        !state::affects(&manifest, &workspace, &at(&["docs", "plans2", "plan.md"]))
            && !state::affects(&manifest, &workspace, &at(&["src", "a.cs"]))
            && !state::affects(&plain, &workspace, &at(&["docs", "plans", "plan.md"]))
    );
    assert!(
        watcher::concerns(&[], &workspace, None) && watcher::concerns(&[], &workspace, Some(&manifest)),
        "a lost watch reads everything again"
    );
    assert!(
        watcher::concerns(&[at(&[".claude", "mighty-styles", "x.json"])], &workspace, None)
            && watcher::concerns(&[at(&[".claude"])], &workspace, None)
    );
    assert!(
        !watcher::concerns(&[at(&["src", "a.cs"]), at(&[".claude", "settings.json"])], &workspace, Some(&manifest))
            && !watcher::concerns(&[at(&["docs", "plans", "plan.md"])], &workspace, None)
    );
    assert!(watcher::concerns(&[at(&["src", "a.cs"]), at(&["docs", "plans", "plan.md"])], &workspace, Some(&manifest)));
    let deck = fixture_style("deck");
    assert!(
        !watcher::concerns(&[at(&["notes", "x.md"])], &workspace, Some(&deck.manifest))
            && !watcher::concerns(&[at(&["notes", "x.md"])], &workspace, Some(&manifest))
    );
}

/// L722 "styles: the change watcher reports file changes, a folder made later and one taken away". C# runs it on Windows only
/// (FileSystemWatcher); this watcher is plain std polling, so it runs on every OS.
#[test]
fn styles_the_change_watcher_reports_file_changes_a_folder_made_later_and_one_taken_away() {
    use std::time::{Duration, Instant};
    let root = Temp::new();
    let folder = root.join("profile").join("styles");
    let file = folder.join("a.json");
    let reports: Arc<Mutex<Vec<Vec<std::path::PathBuf>>>> = Arc::new(Mutex::new(Vec::new()));
    let count = || reports.lock().expect("reports").len();
    let saw = |path: &Path, from: usize| {
        reports.lock().expect("reports")[from..].iter().any(|r| r.iter().any(|p| watcher::touches(p, path) && watcher::touches(path, p)))
    };
    let until = |check: &dyn Fn() -> bool| {
        let start = Instant::now();
        while !check() {
            assert!(start.elapsed() < Duration::from_secs(10), "timed out");
            std::thread::sleep(Duration::from_millis(20));
        }
    };
    let sink = reports.clone();
    let watch =
        watcher::StyleChangeWatcher::new(&folder, move |paths| sink.lock().expect("reports").push(paths), Some(Duration::from_millis(40)));
    {
        let standard = watcher::StyleChangeWatcher::new(&root.0, |_| {}, None);
        assert!(watch.latency() == Duration::from_millis(40) && standard.latency() == Duration::from_millis(500));
    }
    std::thread::sleep(Duration::from_millis(150));
    assert_eq!(count(), 0, "nothing is reported while nothing changes");
    std::fs::create_dir_all(&folder).expect("folder");
    until(&|| saw(&folder, 0));
    let before = count();
    std::fs::write(&file, "{}").expect("write");
    until(&|| saw(&file, before));
    let before = count();
    std::fs::write(&file, "{ }  ").expect("write");
    until(&|| saw(&file, before));
    let before = count();
    for i in 0..40 {
        std::fs::write(folder.join(format!("burst-{i}.json")), "{}").expect("burst");
    }
    until(&|| saw(&folder.join("burst-39.json"), before));
    std::thread::sleep(Duration::from_millis(120));
    assert!(count() - before < 20, "a burst of forty files is gathered into few reports");
    let before = count();
    std::fs::remove_dir_all(&folder).expect("remove");
    until(&|| count() > before);
    std::fs::create_dir_all(&folder).expect("again");
    let before = count();
    until(&|| {
        let _ = std::fs::write(&file, count().to_string());
        saw(&file, before)
    });
    drop(watch);
    std::thread::sleep(Duration::from_millis(120));
    let after = count();
    std::fs::write(&file, "late").expect("late");
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(count(), after, "a dropped watcher reports nothing");
}

/// L730 "styles StateBoundaries".
#[test]
fn styles_state_boundaries() {
    let root = Temp::new();
    let outside = Temp::new();
    let manifest = decoder::decode(&fixture("conformance/valid/with-state-sources.json"), "user", None).expect("manifest");
    std::fs::create_dir_all(root.join("docs").join("plans")).expect("plans");
    std::fs::write(root.join("docs").join("plans").join("plan.md"), "- [x] done\n```\n- [ ] example\n```\n- [ ] remaining").expect("plan");
    std::fs::write(outside.join("private.md"), "private").expect("private");
    let locale = korean();
    assert!(!state::read(&manifest, &root.0, None, None, &locale).files[&0].exists, "old plan cannot advance new pane");
    let minute_ago = mighty_core::time::DateTimeOffset::from_utc_ticks(mighty_core::time::DateTimeOffset::now().utc_ticks() - 600_000_000);
    let reading = state::read(&manifest, &root.0, Some(minute_ago), None, &locale);
    assert!(
        reading.files[&0].exists && !reading.files[&0].all_checked && reading.widgets[0].value == 1 && reading.widgets[0].total == Some(2),
        "real bounded checklist read"
    );
    #[cfg(windows)]
    let linked = std::os::windows::fs::symlink_file(outside.join("private.md"), root.join("escape.md"));
    #[cfg(unix)]
    let linked = std::os::unix::fs::symlink(outside.join("private.md"), root.join("escape.md"));
    if linked.is_ok() {
        assert!(files::read(&root.0, "escape.md", decoder::MAXIMUM_BYTES).is_none(), "symlink outside root refused");
    }
    for path in ["../private.md", "C:\\private.md", "\\\\host\\share", "/private.md", "..\\private.md", "file:stream"] {
        assert!(!decoder::safe_relative_path(path), "Windows escape refused {path}");
    }
    std::fs::write(root.join("large.md"), vec![0u8; decoder::MAXIMUM_BYTES + 1]).expect("large");
    assert!(files::read(&root.0, "large.md", decoder::MAXIMUM_BYTES).is_none(), "oversize state refused");
    let (state_of, widget) = state::parse("{\"value\":10,\"total\":4}", "json", "progressBar", &locale);
    assert!(widget.value == 4 && !state_of.all_checked, "json clamps and never means allChecked");
}

/// L1 (W9-D review): a decision holds `write.lock` through the host's exclusive lock, as C#'s `FileShare.None` does: while another
/// holds it the decision fails and nothing is written.
#[test]
fn styles_a_decision_fails_while_another_process_holds_the_write_lock() {
    /// The host's lock as another process holding it answers.
    struct Held(std::sync::atomic::AtomicBool);
    impl mighty_styles::TrustAccess for Held {
        fn private(&self, path: &Path, directory: bool) -> bool {
            mighty_styles::PlainAccess.private(path, directory)
        }
        fn restrict(&self, _: &Path, _: bool) -> std::io::Result<()> {
            Ok(())
        }
        fn lock(&self, _: &Path) -> std::io::Result<Box<dyn std::any::Any + Send>> {
            if self.0.load(std::sync::atomic::Ordering::SeqCst) { Err(std::io::Error::other("held")) } else { Ok(Box::new(())) }
        }
    }
    let root = Temp::new();
    let workspace = root.join("workspace");
    let profile = root.join("profile");
    std::fs::create_dir_all(&workspace).expect("workspace");
    std::fs::create_dir_all(profile.join("styles")).expect("styles");
    std::fs::write(profile.join("styles").join("corpus-min.json"), fixture("conformance/valid/minimal.json")).expect("write");
    let style = single(&profile, &workspace, "corpus-min");
    let held = Arc::new(Held(std::sync::atomic::AtomicBool::new(true)));
    let store = StyleTrustStore::new(&profile.join("style-trust"), held.clone(), korean());
    assert!(store.decide(&style, "approved").is_err(), "the decision waits for no one: it fails");
    assert!(!profile.join("style-trust").join("approvals.json").exists(), "nothing was written");
    held.0.store(false, std::sync::atomic::Ordering::SeqCst);
    store.decide(&style, "approved").expect("approved once the lock is free");
    assert!(single(&profile, &workspace, "corpus-min").runnable());
}
