//! C# RemoteWireFixtureVerification and RemoteStringsVerification (Verification.cs L842-L855): native/contracts/fixtures/remote-v2/
//! read by the Rust side, every body decoding into its wire type and encoding back to the Mac's own JSON (null values aside), read as
//! strictly as the Mac reads it; the wire events both ways; the Windows-only words. Korean is pinned, as C#'s runner pins it.

use std::collections::BTreeSet;
use std::path::PathBuf;

use mighty_core::graph::{MightyGraphAgent, MightyGraphRun};
use mighty_core::json::Wire;
use mighty_core::model::{BackgroundTask, BackgroundWork, LogEntry, PlanRecord, RegisteredModelEntry, RunSettings, SessionUsage};
use mighty_core::permission::{PLAN_TOOL, ToolPermissionRequest};
use mighty_graph::node::ExecutionGraphNode;
use mighty_platform::FirewallChange;
use mighty_remote::remote::events::{self, PendingPermissions};
use mighty_remote::remote::strings;
use mighty_remote::remote::validation;
use mighty_remote::remote::wire::*;
use mighty_run::model::{RunEvent, StartRunRequest};
use mighty_run::plan_mode::{PlanDecision, outcome};
use mighty_run::questionnaire::UserQuestionnaire;
use mighty_run::strings::{get, set_language};
use serde_json::{Map, Value, json};

fn folder() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../contracts/fixtures/remote-v2")
}

fn bytes(name: &str) -> Vec<u8> {
    std::fs::read(folder().join(name)).unwrap_or_else(|_| panic!("{name} is not in the folder"))
}

fn node(name: &str) -> Value {
    serde_json::from_slice(&bytes(name)).unwrap()
}

/// Nulls removed, so an explicit null and a missing key compare the same (the Mac's rule in RemoteWireFixtureTests).
fn plain(value: &Value) -> Value {
    match value {
        Value::Object(o) => Value::Object(o.iter().filter(|(_, v)| !v.is_null()).map(|(k, v)| (k.clone(), plain(v))).collect()),
        Value::Array(items) => Value::Array(items.iter().map(plain).collect()),
        other => other.clone(),
    }
}

fn same(a: &Value, b: &Value) -> bool {
    plain(a) == plain(b)
}

fn text(value: &Value) -> Vec<u8> {
    serde_json::to_vec(value).unwrap()
}

trait Body: Sized {
    fn decode(body: &[u8]) -> Result<Self, BadBody>;
    fn to_value(&self) -> Value;
}

macro_rules! body {
    ($($t:ty),*) => {$(
        impl Body for $t {
            fn decode(body: &[u8]) -> Result<Self, BadBody> {
                <$t>::decode(body)
            }
            fn to_value(&self) -> Value {
                <$t>::to_value(self)
            }
        }
    )*};
}
body!(
    RemoteWireInfo,
    RemoteWireStart,
    RemoteWireJob,
    RemoteWirePoll,
    RemoteWireSteer,
    RemoteWirePermission,
    RemoteWireQuestions,
    RemoteWirePlan,
    RemoteWireAnswer,
    RemoteWireError
);

fn decode<T: Body>(name: &str) -> T {
    T::decode(&bytes(name)).unwrap_or_else(|_| panic!("{name} does not decode"))
}

fn round_trips<T: Body>(name: &str) {
    let written = decode::<T>(name).to_value();
    assert!(same(&node(name), &written), "{name} changed: {written}");
}

/// "remote wire: every body is the Mac's shape".
#[test]
fn every_body_is_the_macs_shape() {
    round_trips::<RemoteWireInfo>("info.json");
    round_trips::<RemoteWireInfo>("info-darwin.json");
    round_trips::<RemoteWireStart>("start.json");
    round_trips::<RemoteWireStart>("start-codex.json");
    round_trips::<RemoteWireJob>("start-accepted.json");
    round_trips::<RemoteWirePoll>("poll.json");
    round_trips::<RemoteWirePoll>("poll-gap.json");
    round_trips::<RemoteWirePoll>("poll-malformed.json");
    round_trips::<RemoteWirePoll>("poll-requests.json");
    round_trips::<RemoteWireSteer>("steer.json");
    round_trips::<RemoteWirePermission>("permission.json");
    round_trips::<RemoteWireQuestions>("questions.json");
    round_trips::<RemoteWirePlan>("plan-approved.json");
    round_trips::<RemoteWirePlan>("plan-revised.json");
    round_trips::<RemoteWireAnswer>("answer.json");
    round_trips::<RemoteWireAnswer>("answer-declined.json");
    round_trips::<RemoteWireError>("error.json");
    assert!(!decode::<RemoteWireAnswer>("answer-declined.json").accepted && decode::<RemoteWireAnswer>("answer.json").accepted);
    let error = decode::<RemoteWireError>("error.json");
    assert!(error.protocol == Some(protocol::VERSION) && !error.error.is_empty(), "a host's error carries the protocol");
    assert_eq!(node("stop.json"), json!({}), "stop's body is {{}}");
    // The HTTP layer under the routes answers 400/413/431 with {error} alone: read, and written back, without a protocol.
    let bare = json!({"error": "Request Header Fields Too Large"});
    let layer = RemoteWireError::decode(&text(&bare)).unwrap();
    assert!(layer.protocol.is_none() && layer.error == "Request Header Fields Too Large" && layer.to_value() == bare);
    // Poll events are kept as the JSON objects received, so every value is written back as it came.
    for name in ["poll.json", "poll-gap.json", "poll-malformed.json", "poll-requests.json"] {
        assert_eq!(node(name)["events"], decode::<RemoteWirePoll>(name).to_value()["events"], "{name} events change");
    }
    let stamps: Vec<String> =
        decode::<RemoteWirePoll>("poll.json").events.iter().map(|e| Value::Object(e.event.clone()).to_string()).collect();
    assert!(
        stamps.iter().any(|s| s.contains("\"2026-10-17T01:02:04Z\"")) && stamps.iter().any(|s| s.contains("\"2026-10-17T01:02:03.456Z\"")),
        "whole-second and millisecond timestamps both survive"
    );
    // A client built from a pane's request writes the same body the Mac's client does.
    let start = decode::<RemoteWireStart>("start.json");
    let pane = start.to_request("pane-1", "ws-project");
    assert!(same(&node("start.json"), &RemoteWireStart::from(&pane, "ws-project").to_value()), "From(ToRequest(start)) is start");
    let codex = decode::<RemoteWireStart>("start-codex.json");
    assert!(same(&node("start-codex.json"), &RemoteWireStart::from(&codex.to_request("pane-2", "ws-project"), "ws-project").to_value()));
    // What the Mac writes as an explicit null, or leaves out, is pinned here.
    for name in ["start.json", "start-codex.json"] {
        let encoded = decode::<RemoteWireStart>(name).to_value();
        let settings = encoded["request"]["settings"].as_object().unwrap();
        assert!(
            settings.get("maxTurns") == Some(&Value::Null) && settings.get("maxBudgetUsd") == Some(&Value::Null),
            "{name}: explicit nulls"
        );
        assert_eq!(
            node(name)["request"]["settings"],
            encoded["request"]["settings"],
            "{name}: settings are written as the Mac writes them"
        );
        assert!(
            encoded["registeredModels"].is_array() && encoded["request"].get("registeredModels").is_none(),
            "{name}: models beside the request"
        );
    }
    let plain_settings = written(&RunSettings::default());
    assert!(["fastMode", "webSearch", "networkAccess"].iter().all(|k| plain_settings.get(*k).is_none()));
    let codex_request = codex.to_value()["request"].clone();
    assert!(codex_request.get("attachments").is_none() && codex_request.get("resumeId").is_none() && codex.registered_models.is_empty());
    let bare_start = RemoteWireStart::from(&StartRunRequest::new("pane-2", "local-ws", "shell", "dir"), "ws-project").to_value();
    assert_eq!(bare_start["registeredModels"], json!([]), "registeredModels is always sent, even empty");
    assert!(bare_start["request"].get("registeredModels").is_none(), "the request itself never carries registeredModels");
    let mut no_files = StartRunRequest::new("pane-3", "local-ws", "claude", "hi");
    no_files.attachments = Some(vec![]);
    assert!(RemoteWireStart::from(&no_files, "ws-project").to_value()["request"].get("attachments").is_none(), "no attachments is no key");
    // Text goes out as the Mac's JSONEncoder writes it: non-ASCII and HTML characters as they are. The remote wire only.
    let words = "<b>Tom & Jerry's</b> + caf\u{e9} \u{d55c}\u{ae00}";
    assert_eq!(String::from_utf8(to_bytes(&Value::String(words.into()))).unwrap(), format!("\"{words}\""));
    assert_ne!(mighty_core::json::escape(words), format!("\"{words}\""), "only the remote wire writes text unescaped");
    for name in ["info.json", "info-darwin.json"] {
        assert!(!String::from_utf8(to_bytes(&decode::<RemoteWireInfo>(name).to_value())).unwrap().contains("\\u"), "{name} has no \\u");
    }
    let mut many = StartRunRequest::new("pane-4", "local-ws", "claude", "hi");
    many.registered_models = (0..80)
        .map(|i| RegisteredModelEntry { name: Some(format!("model-{i}")), supports_effort: false, supported_effort_levels: None })
        .collect();
    assert_eq!(RemoteWireStart::from(&many, "ws-project").registered_models.len(), 64, "at most 64 registered models");
}

/// Files the index does not name as a route's body: other bodies of the same routes (the Mac's RemoteWireFixtureTests.auxiliary).
const AUXILIARY: [&str; 10] = [
    "endpoints.json",
    "info-darwin.json",
    "start-codex.json",
    "poll-gap.json",
    "poll-malformed.json",
    "poll-requests.json",
    "plan-approved.json",
    "answer-declined.json",
    "graph-run-event.json",
    "state-references.json",
];

/// Every file these checks (and mighty-core's state checks) read: C#'s embedded resources.
const READ: [&str; 21] = [
    "answer-declined.json",
    "answer.json",
    "endpoints.json",
    "error.json",
    "graph-run-event.json",
    "info-darwin.json",
    "info.json",
    "permission.json",
    "plan-approved.json",
    "plan-revised.json",
    "poll-gap.json",
    "poll-malformed.json",
    "poll-requests.json",
    "poll.json",
    "questions.json",
    "start-accepted.json",
    "start-codex.json",
    "start.json",
    "state-references.json",
    "steer.json",
    "stop.json",
];

/// "remote wire: the index names every route and every file is accounted for".
#[test]
fn the_index_names_every_route_and_every_file_is_accounted_for() {
    let index = node("endpoints.json");
    let entries = index.as_array().unwrap();
    let routes: BTreeSet<&str> = entries.iter().filter_map(|e| e["name"].as_str()).collect();
    assert_eq!(routes, BTreeSet::from(["info", "start", "events", "stop", "steer", "permission", "questions", "plan", "refused"]));
    let mut named = BTreeSet::new();
    for entry in entries {
        assert!(entry["path"].as_str().is_some_and(|p| p.starts_with(protocol::PATH_PREFIX)), "every route is under /v2/: {entry}");
        for key in ["request", "response"] {
            if let Some(name) = entry[key].as_str() {
                named.insert(name.to_string());
                assert!(folder().join(name).is_file(), "{name} is in the folder");
            }
        }
    }
    let files: BTreeSet<String> =
        std::fs::read_dir(folder()).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
    let auxiliary: BTreeSet<String> = AUXILIARY.iter().map(|s| s.to_string()).collect();
    assert!(named.is_disjoint(&auxiliary), "no route's body is also listed as another body");
    let accounted: BTreeSet<String> = named.union(&auxiliary).cloned().collect();
    assert_eq!(files, accounted, "unaccounted files");
    let read: BTreeSet<String> = READ.iter().map(|s| s.to_string()).collect();
    assert_eq!(read, files, "the checks read every file of the folder and nothing else");
}

/// "remote wire: every sample is accepted by the Windows checks".
#[test]
fn every_sample_is_accepted_by_the_windows_checks() {
    let info = decode::<RemoteWireInfo>("info.json");
    validation::info(&info).unwrap();
    assert!(info.runtime.platform == "win32" && info.workspaces.iter().all(validation::shared_workspace));
    let darwin = decode::<RemoteWireInfo>("info-darwin.json");
    validation::info(&darwin).unwrap();
    let claude_model = &darwin.runtime.providers.iter().find(|p| p.id == "claude").unwrap().model_catalog.models[0];
    assert!(
        darwin.runtime.platform == "darwin"
            && claude_model.resolved_model.as_deref() == Some("claude-opus-4-1")
            && claude_model.supports_effort == Some(true)
            && claude_model.supported_effort_levels.as_ref().is_some_and(|l| l.len() == 5),
        "a Mac host's /info reads in full"
    );
    let mut bad = info.clone();
    bad.runtime.platform = "plan9".into();
    assert!(validation::info(&bad).is_err(), "an unknown platform must be refused");
    let start = decode::<RemoteWireStart>("start.json");
    let request = start.to_request("job-1", "ws-project").validate().unwrap();
    assert!(
        request.permission_mode_override.as_deref() == Some("plan")
            && request.todo_progress.as_ref().and_then(|t| t.items.as_ref()).is_some_and(|i| i.len() == 1)
            && request.phase_models.as_ref().and_then(|p| p.claude_subagent_default.as_deref()) == Some("claude-sonnet-4-5")
            && request.registered_models.len() == 1
            && request.registered_models[0].name.as_deref() == Some("claude-opus-4-1"),
        "the extras ride beside the request"
    );
    let phase = start.phase_models.clone().unwrap();
    assert!(validation::phase_models(&phase), "the sample's phase models are valid");
    let mut bad_model = phase.clone();
    bad_model.claude_main = Some("bad model!".into());
    let mut bad_effort = phase.clone();
    bad_effort.claude_main_effort = Some("ludicrous".into());
    assert!(!validation::phase_models(&bad_model) && !validation::phase_models(&bad_effort), "a bad model or effort is refused");
    let caps = |info: &RemoteWireInfo, id: &str| info.runtime.providers.iter().find(|p| p.id == id).unwrap().capabilities.clone();
    let claude = caps(&info, "claude");
    assert!(validation::capability_problem(&request, &claude).is_none(), "the host supports the sample's settings");
    let mut fast = request.clone();
    fast.settings.as_mut().unwrap().fast_mode = true;
    assert!(validation::capability_problem(&fast, &claude).is_some(), "a setting the host lacks is refused");
    let codex_start = decode::<RemoteWireStart>("start-codex.json");
    let codex = codex_start.to_request("job-2", "ws-project").validate().unwrap();
    let s = codex.settings();
    assert!(
        codex.provider == "codex"
            && s.permission_mode == "onRequest"
            && s.fast_mode
            && s.web_search == "live"
            && s.network_access
            && s.max_turns.is_none()
            && s.max_budget_usd.is_none()
            && codex.registered_models.is_empty(),
        "the Codex sample reads in full"
    );
    let codex_phase = codex_start.phase_models.clone().unwrap();
    assert!(
        validation::phase_models(&codex_phase)
            && codex_phase.codex_main_effort.as_deref() == Some("medium")
            && codex_phase.codex_subagent_effort.as_deref() == Some("low")
            && codex_phase.codex_plan_mode_reasoning_effort.as_deref() == Some("high")
    );
    assert!(
        validation::capability_problem(&codex, &caps(&darwin, "codex")).is_none()
            && validation::capability_problem(&codex, &caps(&info, "codex")).is_none()
    );
    assert!(validation::capability_problem(&codex, &claude).is_some(), "Claude's capabilities refuse the Codex sample");
    let codex_caps = caps(&darwin, "codex");
    let mut no_network = codex_caps.clone();
    no_network.network_access = false;
    let mut no_search = codex_caps.clone();
    no_search.web_search = false;
    let mut manual = codex.clone();
    manual.settings.as_mut().unwrap().permission_mode = "manual".into();
    assert!(
        validation::capability_problem(&codex, &no_network).is_some()
            && validation::capability_problem(&codex, &no_search).is_some()
            && validation::capability_problem(&manual, &codex_caps).is_some()
    );
    assert_eq!(decode::<RemoteWirePlan>("plan-approved.json").decision(), Some(PlanDecision::approve_auto_edit()));
    assert_eq!(decode::<RemoteWirePlan>("plan-revised.json").decision(), Some(PlanDecision::revise("Keep the tests")));
    let mut revised = decode::<RemoteWirePlan>("plan-revised.json");
    revised.feedback = None;
    assert!(revised.decision().is_none(), "revised without feedback is no decision");
    assert_eq!(
        RemoteWirePlan::from("job-1", "perm-3", &PlanDecision::revise("Keep the tests")),
        decode::<RemoteWirePlan>("plan-revised.json")
    );
    assert_eq!(RemoteWirePlan::from("job-1", "perm-3", &PlanDecision::approve_auto_edit()), decode::<RemoteWirePlan>("plan-approved.json"));
    // The answer samples answer the sample requests.
    let requests: Vec<Map<String, Value>> =
        decode::<RemoteWirePoll>("poll-requests.json").events.iter().map(|e| e.event["permission"].as_object().unwrap().clone()).collect();
    assert_eq!(requests.iter().map(|r| r["id"].as_str().unwrap()).collect::<Vec<_>>(), ["perm-2", "perm-3", "perm-4"]);
    let answers = decode::<RemoteWireQuestions>("questions.json");
    let ask = &requests[0];
    assert!(
        ask["toolName"] == "AskUserQuestion"
            && ask["canAnswerQuestions"] == true
            && ask["canAllow"] == false
            && answers.request_id == ask["id"]
            && answers.run_id == ask["runId"]
    );
    let questionnaire = UserQuestionnaire::parse(ask["inputJSON"].as_str().unwrap()).expect("perm-2's inputJSON is a questionnaire");
    let settled = questionnaire.validate_answers(&answers.answers).unwrap();
    assert!(settled.len() == 1 && settled["Which file?"] == "README.md, and the docs", "the sample answers settle the sample questions");
    let plan = &requests[1];
    for name in ["plan-approved.json", "plan-revised.json"] {
        let answer = decode::<RemoteWirePlan>(name);
        assert!(
            plan["toolName"] == PLAN_TOOL
                && plan["canAnswerPlan"] == true
                && plan["canAllow"] == false
                && answer.request_id == plan["id"]
                && answer.run_id == plan["runId"]
                && answer.decision().is_some(),
            "{name} answers perm-3"
        );
    }
    let blocked = &requests[2];
    assert!(
        blocked["reason"].is_string()
            && blocked["blockedPath"].is_string()
            && blocked["canAnswerQuestions"] == false
            && blocked["canAnswerPlan"] == false
    );
    let poll = decode::<RemoteWirePoll>("poll.json");
    let perm1 = poll.events.iter().find(|e| e.event["type"] == "permission").unwrap().event["permission"].clone();
    let permission = decode::<RemoteWirePermission>("permission.json");
    assert!(
        permission.request_id == "perm-1"
            && permission.allow
            && permission.request_id == perm1["id"]
            && permission.run_id == perm1["runId"]
    );
    let key = "A".repeat(43);
    assert!(
        validation::key(Some(&key))
            && !validation::key(Some(&key[..42]))
            && !validation::key(Some(&format!("{key}+")))
            && !validation::key(Some(&"A".repeat(129))),
        "key shape"
    );
    assert!(validation::keys_match(&key, &key) && !validation::keys_match(&key, &format!("{key}B")) && !validation::keys_match("", &key));
    assert!(
        validation::permission_mode("auto", "claude")
            && !validation::permission_mode("auto", "codex")
            && validation::permission_mode("onRequest", "codex")
            && !validation::permission_mode("onRequest", "gemini")
    );
    let failure = |status| RemoteHttpFailure { status, message: "busy".into() };
    assert!(failure(429).transient() && failure(503).transient() && !failure(401).transient(), "429 and 503 are passing failures");
}

/// The JSON of a sample with one change made to it.
fn changed(name: &str, change: impl FnOnce(&mut Value)) -> Vec<u8> {
    let mut value = node(name);
    change(&mut value);
    text(&value)
}

fn refused<T: Body>(body: &[u8], what: &str) {
    assert!(T::decode(body).is_err(), "{what} must be refused: {}", String::from_utf8_lossy(body));
}

fn remove(value: &mut Value, key: &str) {
    value.as_object_mut().unwrap().shift_remove(key);
}

/// "remote wire: bodies are read as strictly as the Mac reads them".
#[test]
fn bodies_are_read_as_strictly_as_the_mac_reads_them() {
    // Member names match exactly, numbers are numbers.
    refused::<RemoteWireAnswer>(br#"{"protocol":"2","accepted":true}"#, "a protocol sent as a string");
    refused::<RemoteWirePoll>(&changed("poll.json", |n| n["cursor"] = json!("11")), "a cursor sent as a string");
    refused::<RemoteWireAnswer>(br#"{"Protocol":2,"accepted":true}"#, "a member spelled in another case");
    refused::<RemoteWireStart>(
        &changed("start.json", |n| {
            let request = n["request"].as_object_mut().unwrap();
            let kind = request.shift_remove("kind").unwrap();
            request.insert("Kind".into(), kind);
        }),
        "a request member spelled in another case",
    );
    // Members the Mac requires are required.
    refused::<RemoteWireAnswer>(br#"{"protocol":2}"#, "an answer without accepted");
    refused::<RemoteWirePoll>(&changed("poll.json", |n| remove(n, "events")), "a poll without events");
    refused::<RemoteWirePoll>(&changed("poll.json", |n| remove(n, "lastCursor")), "a poll without lastCursor");
    refused::<RemoteWireStart>(&changed("start-codex.json", |n| remove(n, "registeredModels")), "a start without registeredModels");
    refused::<RemoteWireStart>(&changed("start.json", |n| remove(&mut n["request"], "model")), "a request without model");
    refused::<RemoteWireStart>(&changed("start.json", |n| remove(&mut n["phaseModels"], "claudeMain")), "phase models without claudeMain");
    refused::<RemoteWireInfo>(&changed("info.json", |n| remove(&mut n["runtime"]["providers"][0], "capabilities")), "no capabilities");
    refused::<RemoteWireInfo>(&changed("info.json", |n| remove(&mut n["runtime"]["providers"][0]["capabilities"], "resume")), "no resume");
    refused::<RemoteWireQuestions>(
        &changed("questions.json", |n| remove(&mut n["answers"]["Which file?"], "selectedOptions")),
        "no options",
    );
    // A null where the Mac's type is not optional is refused, also inside a list or a dictionary.
    refused::<RemoteWireInfo>(&changed("info.json", |n| n["runtime"]["appVersion"] = Value::Null), "a null appVersion");
    refused::<RemoteWireInfo>(&changed("info.json", |n| n["runtime"]["providers"][1]["detail"] = Value::Null), "a null provider detail");
    refused::<RemoteWireInfo>(
        &changed("info-darwin.json", |n| n["runtime"]["providers"][0]["modelCatalog"]["models"][1]["displayName"] = Value::Null),
        "a null model displayName",
    );
    refused::<RemoteWireInfo>(
        &changed("info-darwin.json", |n| remove(&mut n["runtime"]["providers"][0]["modelCatalog"]["models"][1], "description")),
        "a model without description",
    );
    refused::<RemoteWirePoll>(&changed("poll.json", |n| n["events"].as_array_mut().unwrap().push(Value::Null)), "a null entry in events");
    refused::<RemoteWirePoll>(&changed("poll.json", |n| n["events"][0]["event"] = Value::Null), "a null event");
    refused::<RemoteWirePoll>(&changed("poll.json", |n| n["events"][0]["event"] = json!("status")), "an event that is not an object");
    refused::<RemoteWireInfo>(&changed("info.json", |n| n["workspaces"].as_array_mut().unwrap().push(Value::Null)), "a null workspace");
    refused::<RemoteWireInfo>(
        &changed("info.json", |n| {
            n["runtime"]["providers"][0]["capabilities"]["permissionModes"].as_array_mut().unwrap().push(Value::Null)
        }),
        "a null permission mode",
    );
    refused::<RemoteWireStart>(&changed("start.json", |n| n["registeredModels"].as_array_mut().unwrap().push(Value::Null)), "a null model");
    refused::<RemoteWireQuestions>(&changed("questions.json", |n| n["answers"]["Which file?"] = Value::Null), "a null answer");
    // A key repeated in one object is refused while the body is read; keys differing only in case are two keys, as on the Mac.
    refused::<RemoteWireAnswer>(br#"{"protocol":2,"accepted":true,"accepted":false}"#, "a repeated member");
    refused::<RemoteWirePoll>(
        br#"{"protocol":2,"cursor":1,"lastCursor":1,"gap":false,"done":false,"events":[{"cursor":1,"event":{"sessionId":"job-1","type":"status","type":"log"}}]}"#,
        "an event with a repeated key",
    );
    refused::<RemoteWirePoll>(
        br#"{"protocol":2,"cursor":1,"lastCursor":1,"gap":false,"done":false,"events":[{"cursor":1,"event":{"sessionId":"job-1","type":"log","entry":{"id":"a","id":"b"}}}]}"#,
        "an event with a key repeated deeper in it",
    );
    let colliding = RemoteWirePoll::decode(
        br#"{"protocol":2,"cursor":1,"lastCursor":1,"gap":false,"done":false,"events":[{"cursor":1,"event":{"sessionId":"job-1","type":"status","Type":"log","status":"running","entry":{"id":"a","ID":"b"}}}]}"#,
    )
    .unwrap();
    let ev = &colliding.events[0].event;
    assert!(ev["type"] == "status" && ev["Type"] == "log" && ev.len() == 5 && ev["entry"]["id"] == "a" && ev.get("TYPE").is_none());
    // What the Mac's decoders default is defaulted here too.
    let bare = RemoteWireStart::decode(
        br#"{"request":{"sessionId":"p","workspaceId":"w","kind":"claude","input":"hi","model":"default","attachments":[]},"registeredModels":[{"name":"m"}]}"#,
    )
    .unwrap();
    assert!(
        bare.request.provider == "claude"
            && bare.request.settings.effort == "default"
            && bare.request.settings.permission_mode == "manual"
            && bare.request.attachments.is_none()
            && bare.request.resume_id.is_none()
            && !bare.registered_models[0].supports_effort
            && bare.registered_models[0].supported_effort_levels.is_empty(),
        "provider, settings and the model's flags default as on the Mac"
    );
    let null_provider = RemoteWireStart::decode(&changed("start.json", |n| n["request"]["provider"] = Value::Null)).unwrap();
    assert_eq!(null_provider.request.provider, "claude", "a null provider is claude, as on the Mac");
    let written = bare.to_value();
    assert!(
        written["request"]["provider"] == "claude"
            && written["request"]["settings"].is_object()
            && written["request"].get("attachments").is_none()
            && written["registeredModels"][0]["supportedEffortLevels"] == json!([]),
        "the defaults are written as the Mac writes them"
    );
    let no_providers = RemoteWireInfo::decode(&changed("info.json", |n| remove(&mut n["runtime"], "providers"))).unwrap();
    validation::info(&no_providers).unwrap();
    let null_providers = RemoteWireInfo::decode(&changed("info.json", |n| n["runtime"]["providers"] = Value::Null)).unwrap();
    assert!(no_providers.runtime.providers.is_empty() && null_providers.runtime.providers.is_empty(), "runtime.providers is optional");
    assert!(RemoteWireError::decode(br#"{"error":"Payload Too Large"}"#).unwrap().protocol.is_none());
}

fn refused_info(info: &RemoteWireInfo, what: &str) {
    assert!(validation::info(info).is_err(), "{what} must be refused");
}

/// "remote wire: /info refuses what the Mac refuses". C#'s last four cases (nulls built in code, not read) cannot be built here: the
/// Rust records hold no null where the Mac's type is not optional.
#[test]
fn info_refuses_what_the_mac_refuses() {
    let info = decode::<RemoteWireInfo>("info.json");
    validation::info(&info).unwrap();
    let many: Vec<RemoteSharedWorkspace> = (0..=protocol::MAXIMUM_SHARED_WORKSPACES)
        .map(|i| RemoteSharedWorkspace { id: format!("ws-{i}"), name: format!("w{i}"), path: format!("C:\\work\\w{i}") })
        .collect();
    let with = |change: &dyn Fn(&mut RemoteWireInfo)| {
        let mut changed = info.clone();
        change(&mut changed);
        changed
    };
    validation::info(&with(&|i| i.workspaces = many[..protocol::MAXIMUM_SHARED_WORKSPACES].to_vec())).unwrap();
    refused_info(&with(&|i| i.workspaces = many.clone()), "more than 64 shared workspaces");
    refused_info(
        &with(&|i| {
            let mut again = i.workspaces[0].clone();
            again.name = "again".into();
            i.workspaces = vec![i.workspaces[0].clone(), again];
        }),
        "a repeated workspace id",
    );
    refused_info(&with(&|i| i.host_name = "h".repeat(121)), "a host name over 120 characters");
    validation::info(&with(&|i| i.host_name = "h".repeat(120))).unwrap();
    refused_info(&with(&|i| i.host_name = String::new()), "an empty host name");
    refused_info(&with(&|i| i.protocol = 1), "another protocol");
    let claude = info.runtime.providers[0].clone();
    let codex = info.runtime.providers[1].clone();
    refused_info(
        &with(&|i| {
            let mut cursor = claude.clone();
            cursor.id = "cursor".into();
            i.runtime.providers = vec![cursor];
        }),
        "an unknown provider id",
    );
    refused_info(
        &with(&|i| {
            let mut auto = codex.clone();
            auto.capabilities.permission_modes.push("auto".into());
            i.runtime.providers = vec![auto];
        }),
        "auto on codex",
    );
    refused_info(
        &with(&|i| {
            let mut on_request = claude.clone();
            on_request.capabilities.permission_modes = vec!["onRequest".into()];
            i.runtime.providers = vec![on_request];
        }),
        "onRequest on claude",
    );
    refused_info(
        &with(&|i| {
            let mut bad = claude.clone();
            bad.model_catalog.models = vec![bad.model_catalog.models[0].clone()];
            bad.model_catalog.models[0].value = "bad model!".into();
            i.runtime.providers = vec![bad];
        }),
        "an invalid model value",
    );
    refused_info(
        &with(&|i| {
            let third = i.runtime.providers[2].clone();
            i.runtime.providers = vec![claude.clone(), codex.clone(), third, claude.clone(), codex.clone(), claude.clone(), codex.clone()];
        }),
        "more providers than there are",
    );
}

/// "remote wire: the host key never leaves the state". RemoteHostState has no serializer at all; `Debug` is its only print.
#[test]
fn the_host_key_never_leaves_the_state() {
    let key = "k3yK3yk3yK3yk3yK3yk3yK3yk3yK3yk3yK3yk3yK3yQ";
    let state = RemoteHostState {
        enabled: true,
        addresses: vec!["100.101.102.103:43137".into()],
        port: Some(43137),
        key: Some(key.into()),
        workspace_ids: vec!["ws-project".into()],
        active_runs: 1,
        allows_full_access: false,
    };
    assert!(validation::key(Some(key)), "the sample key has the key's shape");
    let text = format!("{state:?}");
    assert!(!text.contains(key), "the key is never printed: {text}");
    assert!(
        text.contains("<redacted>")
            && text.contains("100.101.102.103:43137")
            && text.contains("ws-project")
            && text.contains("active_runs: 1")
    );
    assert!(!format!("{:?}", RemoteHostState { key: None, ..state.clone() }).contains("<redacted>"), "no key prints as none");
    assert!(
        state == state.clone() && state != RemoteHostState { key: Some("other".into()), ..state.clone() },
        "the key takes part in equality"
    );
}

fn wire_event(name: &str, cursor: i64) -> Map<String, Value> {
    decode::<RemoteWirePoll>(name).events.into_iter().find(|e| e.cursor == cursor).unwrap().event
}

fn edited(wire: &Map<String, Value>, change: impl FnOnce(&mut Map<String, Value>)) -> Map<String, Value> {
    let mut copy = wire.clone();
    change(&mut copy);
    copy
}

fn inner<'a>(wire: &'a mut Map<String, Value>, key: &str) -> &'a mut Map<String, Value> {
    wire.get_mut(key).unwrap().as_object_mut().unwrap()
}

fn as_pane(wire: &Map<String, Value>) -> events::Incoming {
    events::read(wire, "job-1", "pane-1", false)
}

/// "remote wire: every wire event reads and writes back".
#[test]
fn every_wire_event_reads_and_writes_back() {
    for name in ["poll.json", "poll-gap.json", "poll-requests.json"] {
        for item in decode::<RemoteWirePoll>(name).events {
            let incoming = events::read(&item.event, "job-1", "pane-1", false);
            assert!(incoming.usable(), "{name} cursor {} is usable", item.cursor);
            if item.event["type"] == "graph" {
                assert!(incoming.ignored && incoming.event.is_none() && incoming.permission.is_none(), "a Mac graph node is ignored");
                continue;
            }
            if let Some(permission) = &incoming.permission {
                assert!(
                    permission.run_id == "pane-1"
                        && incoming.host_run_id.as_deref() == Some("job-1")
                        && permission.input_json == item.event["permission"]["inputJSON"],
                    "a tool request is the pane's, its host run kept"
                );
                let back = ToolPermissionRequest { run_id: incoming.host_run_id.clone().unwrap(), ..permission.clone() };
                let written = events::from_permission(&back, "job-1").map(Value::Object).unwrap_or_default();
                assert!(same(&Value::Object(item.event.clone()), &written), "the permission writes back as the Mac's: {written}");
                continue;
            }
            let value = incoming.event.unwrap();
            assert_eq!(value.session_id, "pane-1", "an event becomes the pane's");
            if value.kind == "status" && value.status.as_deref() == Some("error") {
                assert_eq!(value.reason.as_deref(), Some("authentication"), "auth on the wire is the Windows sign-in reason");
            }
            let written = events::from_run_event(&value, "job-1", false).map(Value::Object).unwrap_or_default();
            assert!(same(&Value::Object(item.event.clone()), &written), "{name} cursor {} writes back: {written}", item.cursor);
        }
    }
    // The question, plan and blocked-path requests as Windows tool requests.
    let requests: Vec<ToolPermissionRequest> =
        decode::<RemoteWirePoll>("poll-requests.json").events.iter().map(|e| as_pane(&e.event).permission.unwrap()).collect();
    let r = &requests[0];
    assert!(
        r.id == "perm-2"
            && r.tool_name == "AskUserQuestion"
            && r.tool_use_id == "toolu_2"
            && r.can_answer_questions
            && !r.can_answer_plan
            && !r.can_allow
            && r.state == "pending"
            && r.summary == "Which file?"
            && r.received_at.as_deref() == Some("2026-10-17T01:05:00.000Z")
            && r.reason.is_none()
            && r.blocked_path.is_none()
            && UserQuestionnaire::parse(&r.input_json).is_some(),
        "perm-2 is a question the pane answers"
    );
    let r = &requests[1];
    assert!(
        r.id == "perm-3"
            && r.tool_name == PLAN_TOOL
            && r.can_answer_plan
            && !r.can_allow
            && !r.can_answer_questions
            && r.plan().as_deref() == Some("# Plan\n1. Read"),
        "perm-3 is a plan the pane answers and can never allow"
    );
    let r = &requests[2];
    assert!(
        r.id == "perm-4"
            && r.tool_name == "Write"
            && r.can_allow
            && !r.can_answer_questions
            && !r.can_answer_plan
            && r.reason.as_deref() == Some("The path is outside the workspace.")
            && r.blocked_path.as_deref() == Some("/Users/example/elsewhere"),
        "perm-4 keeps its reason and blocked path"
    );
    let malformed: Vec<bool> = decode::<RemoteWirePoll>("poll-malformed.json").events.iter().map(|e| as_pane(&e.event).usable()).collect();
    assert_eq!(malformed, [true, false, true], "only the malformed event is unusable");
    assert!(!events::read(&decode::<RemoteWirePoll>("poll.json").events[0].event, "job-2", "pane-1", false).usable(), "another job's");
    let graph_run = node("graph-run-event.json")["event"].as_object().unwrap().clone();
    assert!(!events::read(&graph_run, "job-1", "pane-1", false).usable(), "graph_run is unusable to a client that did not ask");
    let run = events::read(&graph_run, "job-1", "pane-1", true).event.unwrap();
    assert!(run.kind == "graph_run" && run.graph_run.as_ref().and_then(|g| g.id.as_deref()) == Some("gr-1") && run.session_id == "pane-1");
    assert!(events::from_run_event(&run, "job-1", false).is_none(), "graph_run travels only on request");
    assert!(same(&Value::Object(graph_run.clone()), &Value::Object(events::from_run_event(&run, "job-1", true).unwrap())));
    // A tracker node of a Windows host writes the Mac's graph event.
    let graph = wire_event("poll.json", 10);
    let tracker = ExecutionGraphNode::read(&graph["graph"]).unwrap();
    let written = events::from_graph_node(&tracker, "job-1").map(Value::Object).unwrap_or_default();
    assert!(same(&Value::Object(graph.clone()), &written), "a tracker node writes the sample's graph event: {written}");
    // A value the client would refuse does not travel; the host leaves it out.
    let mut log = RunEvent::new("pane-9", "log");
    log.entry = Some(LogEntry {
        id: Some("log-9".into()),
        kind: Some("output".into()),
        text: None,
        timestamp: Some("2026-10-17T01:02:03Z".into()),
        ..LogEntry::default()
    });
    assert!(events::from_run_event(&log, "job-9", false).is_none(), "a log the wire cannot carry is none");
    let bad_node = ExecutionGraphNode::new("node-9".into(), "bad run!".into(), None, "main", "running", "Request".into());
    assert!(events::from_graph_node(&bad_node, "job-9").is_none(), "a graph node the Mac's client would not normalize does not travel");
    // A task's output file is a path on the host: neither a tracker node nor a graph_run carries it.
    let mut task = ExecutionGraphNode::new("node-9".into(), "act-9".into(), Some("node-1".into()), "task", "running", "Task".into());
    task.output_file = Some("/tmp/example/task.output".into());
    let task_node = events::from_graph_node(&task, "job-9").unwrap();
    assert!(task_node["graph"].get("outputFile").is_none() && task_node["graph"]["kind"] == "task", "a task node's output file stays home");
    let task_run = MightyGraphRun {
        id: Some("gr-9".into()),
        input: Some("Request".into()),
        agents: Some(vec![MightyGraphAgent {
            id: Some("t1".into()),
            title: Some("Task".into()),
            kind: Some("task".into()),
            background: Some(true),
            output_file: Some("/tmp/example/task.output".into()),
            ..MightyGraphAgent::default()
        }]),
        ..MightyGraphRun::default()
    };
    let mut run_event = RunEvent::new("pane-9", "graph_run");
    run_event.graph_run = Some(task_run.clone());
    let task_wire = events::from_run_event(&run_event, "job-9", true).unwrap();
    let agent = &task_wire["graphRun"]["agents"][0];
    assert!(agent.get("outputFile").is_none() && agent["id"] == "t1");
    assert_eq!(task_run.agents.unwrap()[0].output_file.as_deref(), Some("/tmp/example/task.output"), "the host's run keeps it");
    let mut plan = RunEvent::new("pane-9", "plan");
    plan.plan = Some(PlanRecord {
        id: Some("plan-9".into()),
        run_id: Some("job-9".into()),
        plan: Some("# Plan".into()),
        received_at: None,
        decided_at: Some("2026-10-17T01:02:03.000Z".into()),
        outcome: Some(outcome::APPROVED_AUTO.into()),
        feedback: None,
        plan_truncated: None,
        graph_run_id: None,
        launch_override: None,
    });
    assert!(events::from_run_event(&plan, "job-9", false).is_none());
    assert!(events::from_run_event(&RunEvent::state("pane-9", "paused", None), "job-9", false).is_none(), "a client would refuse it");
    let quota = events::from_run_event(&RunEvent::state("pane-9", "error", Some("quota")), "job-9", false).unwrap();
    assert!(quota.get("reason").is_none() && quota["sessionId"] == "job-9", "only auth travels as a reason");
    let mut permission_event = RunEvent::new("pane-9", "permission");
    permission_event.permission = Some(requests[2].clone());
    assert!(events::from_run_event(&permission_event, "job-9", false).is_none());
    assert!(
        events::from_run_event(&RunEvent::new("pane-9", "steer"), "job-9", false).is_none(),
        "a type the Mac does not read never travels"
    );
}

/// "remote wire: each wire event is refused alone, as the Mac refuses it".
#[test]
fn each_wire_event_is_refused_alone_as_the_mac_refuses_it() {
    let of = |cursor| wire_event("poll.json", cursor);
    let (status, resume, log, activity, usage) = (of(1), of(2), of(3), of(4), of(5));
    let (permission, plan, todos, background) = (of(6), of(7), of(8), of(9));
    let dance = |a: &mut Map<String, Value>| {
        a.insert("kind".into(), "dance".into());
    };
    let activity_value = activity["activity"].as_object().unwrap().clone();
    let refused: Vec<(&str, Map<String, Value>)> = vec![
        ("no sessionId", edited(&status, |e| drop(e.shift_remove("sessionId")))),
        ("no type", edited(&status, |e| drop(e.shift_remove("type")))),
        ("a number for a sessionId", edited(&status, |e| drop(e.insert("sessionId".into(), json!(1))))),
        ("a null type", edited(&status, |e| drop(e.insert("type".into(), Value::Null)))),
        ("an unknown type", edited(&status, |e| drop(e.insert("type".into(), json!("steer"))))),
        ("a mistyped status", edited(&status, |e| drop(e.insert("status".into(), json!(1))))),
        ("an unknown status", edited(&status, |e| drop(e.insert("status".into(), json!("paused"))))),
        ("a reason other than auth", edited(&status, |e| drop(e.insert("reason".into(), json!("quota"))))),
        ("a mistyped resumeId", edited(&resume, |e| drop(e.insert("resumeId".into(), json!(5))))),
        ("a resume without its id", edited(&resume, |e| drop(e.shift_remove("resumeId")))),
        ("an entry that is not an object", edited(&log, |e| drop(e.insert("entry".into(), json!("text"))))),
        ("a log without its entry", edited(&log, |e| drop(e.shift_remove("entry")))),
        ("an entry without a kind", edited(&log, |e| drop(inner(e, "entry").shift_remove("kind")))),
        ("an entry without text", edited(&log, |e| drop(inner(e, "entry").shift_remove("text")))),
        ("an entry without a timestamp", edited(&log, |e| drop(inner(e, "entry").shift_remove("timestamp")))),
        ("an entry whose text is null", edited(&log, |e| drop(inner(e, "entry").insert("text".into(), Value::Null)))),
        ("an entry whose id is a number", edited(&log, |e| drop(inner(e, "entry").insert("id".into(), json!(3))))),
        ("an entry with a mistyped provider", edited(&log, |e| drop(inner(e, "entry").insert("provider".into(), json!(1))))),
        ("an entry with an unknown provider", edited(&log, |e| drop(inner(e, "entry").insert("provider".into(), json!("llama"))))),
        ("an entry longer than the Mac takes", edited(&log, |e| drop(inner(e, "entry").insert("text".into(), json!("a".repeat(131_073)))))),
        (
            "an entry whose activity is not valid",
            edited(&log, |e| drop(inner(e, "entry").insert("activity".into(), Value::Object(edited(&activity_value, dance))))),
        ),
        ("a mistyped activity on any event", edited(&log, |e| drop(e.insert("activity".into(), json!(5))))),
        ("an activity without its summary", edited(&activity, |e| drop(inner(e, "activity").shift_remove("summary")))),
        ("an activity with a mistyped tool name", edited(&activity, |e| drop(inner(e, "activity").insert("toolName".into(), json!(7))))),
        ("an activity that is not valid", edited(&activity, |e| drop(inner(e, "activity").insert("state".into(), json!("dancing"))))),
        ("an activity event without one", edited(&activity, |e| drop(e.shift_remove("activity")))),
        ("a mistyped permission on any event", edited(&log, |e| drop(e.insert("permission".into(), json!("allow"))))),
        ("a permission without its state", edited(&permission, |e| drop(inner(e, "permission").shift_remove("state")))),
        ("a permission without canAllow", edited(&permission, |e| drop(inner(e, "permission").shift_remove("canAllow")))),
        (
            "a permission with a mistyped canAllow",
            edited(&permission, |e| drop(inner(e, "permission").insert("canAllow".into(), json!("yes")))),
        ),
        ("a mistyped canAnswerPlan", edited(&permission, |e| drop(inner(e, "permission").insert("canAnswerPlan".into(), json!(1))))),
        ("a permission without inputJSON", edited(&permission, |e| drop(inner(e, "permission").shift_remove("inputJSON")))),
        (
            "a permission spelling inputJSON the Windows way",
            edited(&permission, |e| {
                let p = inner(e, "permission");
                let input = p.shift_remove("inputJSON").unwrap();
                p.insert("inputJson".into(), input);
            }),
        ),
        ("a permission with a bad run id", edited(&permission, |e| drop(inner(e, "permission").insert("runId".into(), json!("bad id"))))),
        (
            "a permission with too much input",
            edited(&permission, |e| drop(inner(e, "permission").insert("inputJSON".into(), json!("a".repeat(65_537))))),
        ),
        ("a permission event without one", edited(&permission, |e| drop(e.shift_remove("permission")))),
        ("a plan without decidedAt", edited(&plan, |e| drop(inner(e, "plan").shift_remove("decidedAt")))),
        ("a plan with an unknown outcome", edited(&plan, |e| drop(inner(e, "plan").insert("outcome".into(), json!("maybe"))))),
        ("a todo list with a null item", edited(&todos, |e| drop(inner(e, "todos").insert("items".into(), json!([null]))))),
        (
            "a todo item without its status",
            edited(&todos, |e| drop(inner(e, "todos")["items"][0].as_object_mut().unwrap().shift_remove("status"))),
        ),
        ("background work without turnEnded", edited(&background, |e| drop(inner(e, "background").shift_remove("turnEnded")))),
        (
            "a background task without its status",
            edited(&background, |e| drop(inner(e, "background")["tasks"][0].as_object_mut().unwrap().shift_remove("status"))),
        ),
        ("a mistyped background", edited(&background, |e| drop(e.insert("background".into(), json!(1))))),
        ("an event of another job", edited(&log, |e| drop(e.insert("sessionId".into(), json!("job-2"))))),
    ];
    for (what, wire) in &refused {
        assert!(!as_pane(wire).usable(), "{what} is refused");
    }
    // A usage the Mac's decoder cannot read, or its client cannot normalize, is left out without a word.
    let set = |key: &'static str, value: Value| move |e: &mut Map<String, Value>| drop(inner(e, "usage").insert(key.into(), value));
    let silent: Vec<(&str, Map<String, Value>)> = vec![
        ("a usage event with a mistyped usage", edited(&usage, |e| drop(e.insert("usage".into(), json!("lots"))))),
        ("a usage event the client cannot normalize", edited(&usage, set("provider", json!("llama")))),
        ("a usage event whose count is a string", edited(&usage, set("inputTokens", json!("12")))),
        ("a usage event whose count is not whole", edited(&usage, set("inputTokens", json!(12.5)))),
        ("a usage event whose cost is a string", edited(&usage, set("costUSD", json!("0.01")))),
        ("a usage event whose model is a number", edited(&usage, set("model", json!(4)))),
        ("a usage event without updatedAt", edited(&usage, |e| drop(inner(e, "usage").shift_remove("updatedAt")))),
        ("a usage event without its source", edited(&usage, |e| drop(inner(e, "usage").shift_remove("source")))),
        ("a usage event with a null provider", edited(&usage, set("provider", Value::Null))),
        ("a rate-limit window without a kind", edited(&usage, set("rateLimits", json!([{"percentUsed": 5}])))),
        ("rate limits that are not a list", edited(&usage, set("rateLimits", json!("full")))),
        ("a usage event without one", edited(&usage, |e| drop(e.shift_remove("usage")))),
    ];
    for (what, wire) in &silent {
        let read = as_pane(wire);
        assert!(read.usable() && read.event.is_none() && read.permission.is_none() && read.ignored, "{what} is left out silently");
    }
    assert!(!events::read(&silent[0].1, "job-2", "pane-1", false).usable(), "another job's usage is still unusable");
    let windowed = edited(&usage, |e| {
        let u = inner(e, "usage");
        u.insert("model".into(), Value::Null);
        u.insert("rateLimits".into(), json!([{"kind": "five_hour", "percentUsed": 5}]));
    });
    let read = as_pane(&windowed).event.and_then(|e| e.usage).unwrap();
    assert!(read.input_tokens == Some(1200) && read.rate_limits.as_ref().is_some_and(|l| l.len() == 1 && l[0].kind == "five_hour"));
    // What the Mac's decoder lets go (try?) only leaves that value out; the event still reads.
    let lenient: Vec<(&str, Map<String, Value>)> = vec![
        ("a mistyped usage on a log", edited(&log, |e| drop(e.insert("usage".into(), json!("lots"))))),
        ("a mistyped plan on a log", edited(&log, |e| drop(e.insert("plan".into(), json!(1))))),
        ("a mistyped reason", edited(&of(11), |e| drop(e.insert("reason".into(), json!(5))))),
        ("a mistyped entry activity", edited(&log, |e| drop(inner(e, "entry").insert("activity".into(), json!("busy"))))),
        ("mistyped pictures", edited(&log, |e| drop(inner(e, "entry").insert("images".into(), json!("lots"))))),
        ("an unknown key", edited(&log, |e| drop(e.insert("future".into(), json!(1))))),
        ("a key in another case", edited(&status, |e| drop(e.insert("Type".into(), json!("log"))))),
        (
            "a permission without the answer flags",
            edited(&permission, |e| {
                inner(e, "permission").shift_remove("canAnswerQuestions");
                inner(e, "permission").shift_remove("canAnswerPlan");
            }),
        ),
    ];
    for (what, wire) in &lenient {
        assert!(as_pane(wire).usable(), "{what} still reads");
    }
    let foreign = edited(&status, |e| {
        let entry = edited(log["entry"].as_object().unwrap(), |entry| {
            drop(entry.insert("activity".into(), Value::Object(edited(&activity_value, dance))))
        });
        e.insert("entry".into(), Value::Object(entry));
    });
    let read = as_pane(&foreign).event.unwrap();
    assert!(
        read.kind == "status" && read.entry.as_ref().is_some_and(|e| e.activity.is_none()),
        "only left out on an event other than a log"
    );
    let long = edited(&activity, |e| drop(inner(e, "activity").insert("durationMs".into(), json!(1e20))));
    let read = as_pane(&long).event.and_then(|e| e.activity).unwrap();
    assert!(read.id == "act-1" && read.duration_ms.is_none(), "an out-of-range duration reads as none");
    let reason = as_pane(&lenient[2].1).event.unwrap();
    assert!(reason.kind == "status" && reason.status.as_deref() == Some("completed") && reason.reason.is_none());
    let busy = as_pane(&lenient[3].1).event.and_then(|e| e.entry).unwrap();
    assert!(busy.activity.is_none() && busy.text.as_deref() == Some("Reading README.md."));
    let cased = as_pane(&lenient[6].1).event.unwrap();
    assert!(cased.kind == "status" && cased.status.as_deref() == Some("running"));
    let flags = as_pane(&lenient[7].1).permission.unwrap();
    assert!(!flags.can_answer_questions && !flags.can_answer_plan && flags.can_allow, "the lenient values read as the Mac reads them");
    // A picture names a file on the host: the entry reads as its text alone.
    let picture = edited(&log, |e| {
        let entry = inner(e, "entry");
        entry.insert("kind".into(), "image".into());
        entry.insert(
            "images".into(),
            json!([{"hash": "a".repeat(64), "mediaType": "image/png", "width": 1, "height": 1, "bytes": 10, "source": "tool"}]),
        );
    });
    let read = as_pane(&picture).event.and_then(|e| e.entry).unwrap();
    assert!(
        read.kind() == "system" && read.images.is_none() && read.text.as_deref() == Some("Reading README.md."),
        "a host's picture reads as its text"
    );
    // Read never fails loudly, whatever the object holds.
    for odd in [
        "{}",
        r#"{"sessionId":{},"type":[]}"#,
        r#"{"sessionId":"job-1","type":"log","entry":[1,2]}"#,
        r#"{"sessionId":"job-1","type":"permission","permission":{"inputJSON":{}}}"#,
        r#"{"sessionId":"job-1","type":"background","background":{"tasks":[1],"turnEnded":true}}"#,
    ] {
        let wire: Map<String, Value> = serde_json::from_str(odd).unwrap();
        assert!(!as_pane(&wire).usable(), "{odd} is unusable");
    }
}

fn whole_seconds(value: &str) -> bool {
    value.len() == 20 && value.ends_with('Z') && value.as_bytes()[10] == b'T' && !value.contains('.')
}

fn milliseconds(value: &str) -> bool {
    value.len() == 24 && value.ends_with('Z') && value.as_bytes()[19] == b'.'
}

/// "remote wire: timestamps, images and pending requests are bounded".
#[test]
fn timestamps_images_and_pending_requests_are_bounded() {
    // A Windows host writes each time in the form the Mac writes it.
    let log = events::from_run_event(&RunEvent::log("job-9", "output", "hi", None), "job-9", false).unwrap();
    let stamp = log["entry"]["timestamp"].as_str().unwrap();
    assert!(whole_seconds(stamp), "a log time travels in whole seconds: {stamp}");
    let now = mighty_core::time::now_o();
    let mut request = ToolPermissionRequest::new("r1", "pane-9", "t1", "Bash", "{}", "ls");
    request.received_at = Some(now.clone());
    let wire = events::from_permission(&request, "job-9").unwrap();
    let p = &wire["permission"];
    assert!(
        milliseconds(p["receivedAt"].as_str().unwrap())
            && p["runId"] == "job-9"
            && p["inputJSON"].is_string()
            && p.get("inputJson").is_none()
            && wire["sessionId"] == "job-9",
        "a request travels as the Mac's: {wire:?}"
    );
    let mut planned = RunEvent::new("pane-9", "plan");
    planned.plan = Some(PlanRecord {
        id: Some("plan-9".into()),
        run_id: Some("job-9".into()),
        plan: Some("# Plan".into()),
        received_at: Some(now.clone()),
        decided_at: Some(now.clone()),
        outcome: Some(outcome::APPROVED_AUTO.into()),
        feedback: None,
        plan_truncated: None,
        graph_run_id: None,
        launch_override: None,
    });
    let planned = events::from_run_event(&planned, "job-9", false).unwrap();
    assert!(milliseconds(planned["plan"]["receivedAt"].as_str().unwrap()) && milliseconds(planned["plan"]["decidedAt"].as_str().unwrap()));
    let task = |started: &str, ended: Option<&str>| BackgroundTask {
        id: Some("task-9".into()),
        kind: Some("shell".into()),
        description: Some("npm test".into()),
        started_at: Some(started.into()),
        status: Some(if ended.is_some() { "completed" } else { "running" }.into()),
        tool_use_id: None,
        summary: None,
        ended_at: ended.map(str::to_string),
    };
    let mut work = RunEvent::new("pane-9", "background");
    work.background = Some(BackgroundWork { tasks: Some(vec![task(&now, Some(&now))]), turn_ended: false });
    let work = events::from_run_event(&work, "job-9", false).unwrap();
    let t = &work["background"]["tasks"][0];
    assert!(milliseconds(t["startedAt"].as_str().unwrap()) && milliseconds(t["endedAt"].as_str().unwrap()), "background times in ms");
    let mut used = RunEvent::new("pane-9", "usage");
    used.usage = SessionUsage::from_json(
        &json!({"provider": "claude", "source": "claude.stream-json", "tokenScope": "run", "inputTokens": 1, "updatedAt": now}),
    );
    assert!(used.usage.is_some());
    let used = events::from_run_event(&used, "job-9", false).unwrap();
    assert!(whole_seconds(used["usage"]["updatedAt"].as_str().unwrap()), "a usage time travels in whole seconds");
    let mut graph = ExecutionGraphNode::new("node-9".into(), "act-9".into(), Some("node-1".into()), "agent", "running", "Agent".into());
    graph.updated_at = now.clone();
    graph.entries = vec![RunEvent::log("job-9", "assistant", "hi", None).entry.unwrap()];
    let graph = events::from_graph_node(&graph, "job-9").unwrap();
    assert!(
        graph["type"] == "graph"
            && graph["sessionId"] == "job-9"
            && whole_seconds(graph["graph"]["updatedAt"].as_str().unwrap())
            && whole_seconds(graph["graph"]["entries"][0]["timestamp"].as_str().unwrap()),
        "graph times travel in whole seconds"
    );
    // A time already in one of the Mac's forms travels as it is.
    let mut kept = RunEvent::new("pane-9", "background");
    kept.background = Some(BackgroundWork { tasks: Some(vec![task("2026-10-17T01:02:08Z", None)]), turn_ended: false });
    let kept = events::from_run_event(&kept, "job-9", false).unwrap();
    let mut kept_log = RunEvent::new("pane-9", "log");
    kept_log.entry = Some(LogEntry::new("log-9".into(), "output", "hi".into(), "2026-10-17T01:02:03.456Z".into(), None));
    let kept_log = events::from_run_event(&kept_log, "job-9", false).unwrap();
    assert!(
        kept["background"]["tasks"][0]["startedAt"] == "2026-10-17T01:02:08Z"
            && kept_log["entry"]["timestamp"] == "2026-10-17T01:02:03.456Z"
    );
    assert_eq!(events::mac_timestamp("2026-10-17T10:02:03.1234567+09:00", true).as_deref(), Some("2026-10-17T01:02:03.123Z"));
    assert_eq!(events::mac_timestamp("2026-10-17T10:02:03.9999999+09:00", false).as_deref(), Some("2026-10-17T01:02:03Z"));
    assert!(events::mac_timestamp("not a time", true).is_none() && events::mac_timestamp("2026-10-17", true).is_none());
    let said =
        events::from_run_event(&RunEvent::log("job-9", "output", "2026-10-17T10:02:03.1234567+09:00", None), "job-9", false).unwrap();
    assert_eq!(said["entry"]["text"], "2026-10-17T10:02:03.1234567+09:00", "a time inside text is never rewritten");
    let mut picture = RunEvent::log("job-9", "image", "a picture", None);
    let image = mighty_core::model::AgentImageRef::read(
        &json!({"hash": "h", "mediaType": "image/png", "width": 1, "height": 1, "bytes": 10, "source": "tool"}),
    )
    .unwrap();
    picture.entry.as_mut().unwrap().images = Some(vec![Some(image)]);
    assert!(events::from_run_event(&picture, "job-9", false).unwrap()["entry"].get("images").is_none(), "pictures stay home");
    let mut assistant = picture.entry.clone().unwrap();
    assistant.kind = Some("assistant".into());
    let mut with_picture =
        ExecutionGraphNode::new("node-9".into(), "act-9".into(), Some("node-1".into()), "agent", "running", "Agent".into());
    with_picture.entries = vec![assistant];
    assert!(events::from_graph_node(&with_picture, "job-9").unwrap()["graph"]["entries"][0].get("images").is_none());
    let pending = PendingPermissions::default();
    let mut evicted = vec![];
    for i in 0..40 {
        evicted.extend(pending.admit(&ToolPermissionRequest::new(&format!("r{i}"), "pane-1", &format!("t{i}"), "Bash", "{}", "ls")));
    }
    assert_eq!(evicted.iter().map(|r| r.id.clone()).collect::<Vec<_>>(), (0..8).map(|i| format!("r{i}")).collect::<Vec<_>>());
    assert!(evicted.iter().all(|r| r.state == "cancelled"), "the oldest are withdrawn");
    assert!(pending.count("pane-1") == protocol::MAXIMUM_PENDING_PERMISSIONS);
    assert!(
        pending.admit(&ToolPermissionRequest::new("r1", "pane-2", "t1", "Bash", "{}", "ls")).is_empty() && pending.count("pane-2") == 1
    );
    let mut settled = ToolPermissionRequest::new("r39", "pane-1", "t39", "Bash", "{}", "ls");
    settled.state = "allowed".into();
    assert!(pending.admit(&settled).is_empty() && pending.count("pane-1") == 31, "a settled request leaves the count");
    pending.forget("pane-1");
    assert!(pending.count("pane-1") == 0 && pending.count("pane-2") == 1, "a finished run forgets its requests");
}

/// "remote strings: every Windows-only string resolves".
#[test]
fn every_windows_only_string_resolves() {
    set_language("ko");
    let values = [
        ("localTailscaleOff", strings::local_tailscale_off()),
        ("connectedSaved", strings::connected_saved()),
        ("connectedMemory", strings::connected_memory()),
        ("hostLocalOnly", strings::host_local_only()),
        ("hostNoTailscale", strings::host_no_tailscale()),
        ("notLocal", strings::not_local()),
        ("paneUnavailable", strings::pane_unavailable()),
        ("settingsTailscaleOff", strings::settings_tailscale_off()),
        ("settingsShareNote", strings::settings_share_note()),
        ("settingsFirewallNote", strings::settings_firewall_note()),
        ("settingsScopeKey", strings::settings_scope_key()),
        ("allowFullAccess", strings::settings_allow_full_access_description()),
        ("keyCopied", strings::key_copied()),
        ("keyCopyFailed", strings::key_copy_failed()),
        ("firewallAdd", strings::firewall_add()),
        ("firewallRemove", strings::firewall_remove()),
        ("firewallUpdate", strings::firewall_update()),
        ("firewallNeedsUpdate", strings::firewall_needs_update()),
        ("firewallRemoveClosesBoth", strings::firewall_remove_closes_both()),
        ("sessionInfoHostOS", strings::session_info_host_os()),
        ("timeout", strings::timeout()),
        ("savedUnreadable", strings::saved_unreadable()),
        ("hostCommandTooLong", strings::host_command_too_long()),
        ("hostAgentNeedsFullAccess", strings::host_agent_needs_full_access("codex")),
    ];
    for (name, value) in &values {
        assert!(!value.is_empty() && !value.starts_with("remote."), "{name} has no text: {value}");
    }
    assert!(strings::host_local_only().contains("이 PC"), "the Windows host says 이 PC, never 이 Mac");
    let note = strings::command_note("studio-pc", Some("linux"));
    assert!(note.contains("studio-pc") && note.contains("Linux") && !note.contains('{'), "the command note names the host and its OS");
    for (provider, name) in [("codex", "Codex"), ("gemini", "Antigravity")] {
        let words = strings::host_agent_needs_full_access(provider);
        assert!(words.contains(name) && !words.contains('{') && words != get("remote.host.error.fullAccessOff"), "names {name}");
    }
    assert!(strings::firewall_failed(&FirewallChange::Applied).is_none(), "a change that happened has no failure words");
    assert_eq!(strings::firewall_failed(&FirewallChange::Declined), Some(get("mobileRemote.direct.windows.firewall.cancelled")));
    let timeout = strings::firewall_failed(&FirewallChange::Failed("timeout".into())).unwrap();
    assert!(!timeout.is_empty() && !timeout.contains("timeout"), "a raw code never reaches the words: {timeout}");
}

/// "remote strings: host OS labels cover every platform".
#[test]
fn host_os_labels_cover_every_platform() {
    set_language("ko");
    assert!(
        strings::host_os(Some("darwin")) == "macOS"
            && strings::host_os(Some("win32")) == "Windows"
            && strings::host_os(Some("linux")) == "Linux"
    );
    assert!(strings::host_os(None) == strings::host_os(Some("plan9")) && !strings::host_os(None).is_empty(), "unknown reads as unknown");
    assert_eq!(strings::host_with_os("my-mac", Some("darwin")), "my-mac · macOS");
}

/// C# `RemoteModelListBoundVerification.Catalog`: a "default" row, then `count - 1` of the prefix's.
fn long_catalog(prefix: &str, count: usize) -> Value {
    let mut models = vec![json!({"value": "default", "displayName": "Default", "description": ""})];
    models.extend(
        (1..count)
            .map(|i| json!({"value": format!("{prefix}{i}"), "displayName": format!("{prefix}{i}"), "description": format!("row {i}")})),
    );
    json!({"source": "cli", "models": models, "detail": format!("{prefix} list")})
}

/// info-darwin.json as an older Mac host sends it with long lists: Claude's (both fields) 140, Codex's 200 and pi's `pi`.
fn older_host_info(pi: usize) -> RemoteWireInfo {
    let mut root = node("info-darwin.json");
    root["runtime"]["modelCatalog"] = long_catalog("claude-m", 140);
    root["runtime"]["providers"][0]["modelCatalog"] = long_catalog("claude-m", 140);
    root["runtime"]["providers"][1]["modelCatalog"] = long_catalog("gpt-m", 200);
    root["runtime"]["providers"].as_array_mut().unwrap().push(json!({
        "id": "pi", "name": "pi", "available": true, "detail": "", "modelCatalog": long_catalog("anthropic/m", pi),
        "capabilities": {"effort": true, "permissionModes": ["manual", "fullAccess"], "maxTurns": false, "maxBudgetUsd": false, "resume": true},
    }));
    RemoteWireInfo::decode(&text(&root)).unwrap()
}

/// "remote model lists: an older host's long lists are cut to what a local read keeps".
#[test]
fn an_older_hosts_long_lists_are_cut_to_what_a_local_read_keeps() {
    let info = older_host_info(600);
    assert!(validation::info(&info).is_err(), "unbounded, the info does not pass: the client cuts first");
    let kept = RemoteWireInfo { runtime: validation::bounded(&info.runtime), ..info.clone() };
    validation::info(&kept).unwrap();
    let count = |id: &str| kept.runtime.providers.iter().find(|p| p.id == id).unwrap().model_catalog.models.len();
    assert!(
        count("claude") == 129
            && count("codex") == 129
            && count("gemini") == 1
            && count("pi") == 513
            && kept.runtime.model_catalog.as_ref().map(|c| c.models.len()) == Some(129),
        "each list cut to its bound"
    );
    let pi = &kept.runtime.providers.iter().find(|p| p.id == "pi").unwrap().model_catalog;
    let expected: Vec<String> = std::iter::once("default".to_string()).chain((1..=512).map(|i| format!("anthropic/m{i}"))).collect();
    assert!(
        pi.models.iter().map(|m| m.value.clone()).collect::<Vec<_>>() == expected
            && pi.detail == "anthropic/m list"
            && pi.models[512].description == "row 512"
    );
    let shorter = older_host_info(300);
    let whole = validation::bounded(&shorter.runtime);
    let pick = |r: &mighty_run::model::RuntimeInfo| r.providers.iter().find(|p| p.id == "pi").unwrap().model_catalog.clone();
    assert_eq!(pick(&whole), pick(&shorter.runtime), "a list within its bound is left whole");
    validation::info(&RemoteWireInfo { runtime: whole, ..shorter }).unwrap();
}

/// "remote model lists: the bound is what each read of its CLI keeps".
#[test]
fn the_bound_is_what_each_read_of_its_cli_keeps() {
    use mighty_run::providers;
    let claude: Vec<Value> = (1..=200).map(|i| json!({"value": format!("claude-m{i}")})).collect();
    let codex: Vec<Value> = (1..=200).map(|i| json!({"model": format!("gpt-m{i}")})).collect();
    assert_eq!(providers::claude_catalog(&claude).models.len(), validation::maximum_models("claude"), "claude");
    assert_eq!(providers::codex_catalog(&codex).models.len(), validation::maximum_models("codex"), "codex");
    let agy = (1..=200).map(|i| format!("gemini-m{i}\tGemini {i}")).collect::<Vec<_>>().join("\n");
    assert_eq!(providers::antigravity_catalog(&agy).models.len(), validation::maximum_models("gemini"), "gemini");
    let pi = (1..=600).map(|i| format!("anthropic m{i} 200K 64K yes yes")).collect::<Vec<_>>().join("\n");
    assert_eq!(providers::pi_catalog(&pi).models.len(), validation::maximum_models("pi"), "pi");
    // A kept list is read back to the longest of them, whatever its provider: the host cuts it to its own.
    assert_eq!(mighty_run::kept_lists::maximum_models(), validation::maximum_models("opencode"), "kept lists");
}
