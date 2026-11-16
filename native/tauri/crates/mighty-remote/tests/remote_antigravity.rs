//! C# AntigravityPermissionVerification's remote checks (Verification.cs L627, L633): the Mac's `/v2` samples in the shared fixture
//! (native/contracts/fixtures/antigravity-permissions.json `remote`), and a remote pane's host putting a card's rules in its own agy
//! settings while refusing forged ones (the fake agy over the Windows process host).

mod support;

use std::path::PathBuf;

use mighty_remote::remote::events;
use mighty_remote::remote::wire::*;
use mighty_run::model::RunEvent;
use mighty_run::strings::set_language;
use serde_json::Value;

fn cards() -> Value {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../contracts/fixtures/antigravity-permissions.json");
    serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
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

/// "antigravity cards: the /v2 samples are the Mac's own and pass the client's checks": read, checked and written back by the wire as
/// the Mac writes them.
#[test]
fn the_remote_samples_are_the_macs_own_and_pass_the_clients_checks() {
    set_language("en");
    let remote = cards()["remote"].clone();
    let poll_node = remote["pollEvent"].clone();
    let poll = RemoteWirePoll::decode(&serde_json::to_vec(&poll_node).unwrap()).unwrap();
    assert!(same(&poll_node, &poll.to_value()), "the poll writes back as it came");
    let read: Vec<_> = poll.events.iter().map(|e| events::read(&e.event, "job-1", "pane", false)).collect();
    assert!(read.iter().all(|r| !r.ignored && r.event.is_some()), "every event is one a client uses");
    let first = read[0].event.as_ref().unwrap();
    let card = first.antigravity_permission.clone().expect("no card");
    assert!(
        first.session_id == "pane"
            && card.rules() == ["command(curl -s https://example.com | grep -i '<title>')"]
            && card.actions[1].rule.is_none()
    );
    // A host here writes the same event for the same card.
    let mut event = RunEvent::new("job-1", "antigravityPermission");
    event.antigravity_permission = Some(card.clone());
    let written = events::from_run_event(&event, "job-1", false).expect("the card travels");
    assert!(
        same(&Value::Object(poll.events[0].event.clone()), &Value::Object(written.clone())),
        "a host here writes the Mac's event: {written:?}"
    );
    let start_node = remote["start"].clone();
    let start = RemoteWireStart::decode(&serde_json::to_vec(&start_node).unwrap()).unwrap();
    assert!(same(&start_node, &start.to_value()), "the start writes back as it came");
    let grant = start.antigravity_grant.clone().expect("a grant");
    assert!(
        grant.is_valid() && grant.temporary && grant.request_id == card.id && grant.rules == card.rules(),
        "the grant names the card's rules"
    );
    assert!(start.request.provider == "gemini" && start.request.resume_id.is_some(), "a continuation of the same conversation");
    let mut pane = start.to_request("pane-3", "ws-project");
    pane.antigravity_grant = Some(grant);
    assert!(same(&start_node, &RemoteWireStart::from(&pane, "ws-project").to_value()), "a client here writes the Mac's start");
}

/// "antigravity cards: a remote pane's host puts the rules in its own settings and refuses forged ones" (the Mac's
/// aRemotePanesHostPutsTheRulesInItsOwnSettingsForTheContinuation): the host reports the card, refuses a rule it never reported (403)
/// and a card it never reported and changes nothing, holds the card's own rule while the continuation runs and takes it out after,
/// refuses the same grant again, and refuses Always for a once-only rule (409). The fake agy needs the Windows process host.
#[cfg(windows)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_remote_panes_host_puts_the_rules_in_its_own_settings_and_refuses_forged_ones() {
    use std::sync::Arc;
    use std::time::Duration;

    use mighty_core::locale::Locale;
    use mighty_core::model::AppSnapshot;
    use mighty_core::store::Store;
    use mighty_platform::fake::{FakeNetwork, MemoryVault};
    use mighty_remote::remote::host::{HostOptions, RemoteHost};
    use mighty_run::antigravity::AntigravityRuleLeases;
    use mighty_run::antigravity::permissions::{AllowGrant, PermissionDecision, continuation};
    use mighty_run::model::{CliCommand, StartRunRequest};
    use mighty_run::providers::{Finder, ProviderCatalog};
    use mighty_run::strings::get;
    use support::fake_host::*;

    const CONVERSATION: &str = "11111111-2222-4333-8444-555555555555";
    set_language("en");
    let root = temp("agy-remote");
    let settings = root.join(".gemini").join("antigravity-cli").join("settings.json");
    std::fs::create_dir_all(settings.parent().unwrap()).unwrap();
    let real = cards()["settings"]["edits"][0]["before"].as_str().unwrap().to_string();
    std::fs::write(&settings, &real).unwrap();
    let text = || std::fs::read_to_string(&settings).unwrap_or_default();
    let record = root.join("agy-record").to_string_lossy().into_owned();
    let command = CliCommand::new(
        env!("CARGO_BIN_EXE_mighty-remote-fake-cli"),
        vec!["--fake-cli".into(), "gemini".into(), record, "--agy-denied".into(), settings.to_string_lossy().into_owned()],
        Some("2.1.271"),
    );
    let finder: Finder = Arc::new(move |_| {
        let found = Some(command.clone());
        Box::pin(async move { found })
    });
    let environment = Arc::new(mighty_platform::fake::FakeShellEnvironment::new(std::env::vars().collect()));
    let catalog = Arc::new(ProviderCatalog::new(Arc::new(mighty_platform_windows::WindowsProcessHost::new()), environment, Some(finder)));

    let mut store = Store::new(root.join("host"), None, true);
    store.load(&Locale::new("en")).unwrap();
    std::fs::create_dir_all(root.join("project")).unwrap();
    let shared = store.approve_local(&root.join("project")).unwrap();
    store.save(&AppSnapshot { workspaces: Some(vec![shared.clone()]), ..AppSnapshot::default() }).unwrap();
    let store: SharedStore = Arc::new(std::sync::Mutex::new(store));
    let mut reading = runtime("info.json");
    for provider in &mut reading.providers {
        if provider.id == "gemini" {
            provider.available = true;
            provider.detail = String::new();
        }
    }
    let rules = AntigravityRuleLeases::new(&settings, Some(&root.join("host").join("antigravity-temporary-rules.json")), None, None);
    let (listed, resolving) = (store.clone(), store.clone());
    let host = RemoteHost::new(HostOptions {
        workspaces: Arc::new(move || listed.lock().unwrap().snapshot.workspaces().to_vec()),
        resolve_local: Arc::new(move |id: String| {
            let found = resolve_local(&resolving, &id);
            Box::pin(async move { found })
        }),
        runtime: Arc::new(move || {
            let value = reading.clone();
            Box::pin(async move { value })
        }),
        create_runner: Arc::new(move |sinks| {
            let mut options = mighty_run::manager::RunManagerOptions::new("", sinks.workspace.clone(), sinks.event.clone());
            options.permission_requested = Some(sinks.permission.clone());
            Arc::new(mighty_run::manager::RunManager::new(catalog.clone(), options))
        }),
        host_name: "studio-pc".into(),
        admission: Arc::new(|_| None),
        runs_changed: Arc::new(|_| {}),
        allow_loopback_for_tests: true,
        heartbeat: protocol::HEARTBEAT_TIMEOUT,
        addresses: None,
        clock: mighty_remote::remote::host::monotonic(),
        windows_host: true,
        decodes_paths: false,
        graph_runs: true,
        antigravity_rules: Some(rules.clone()),
        system_folder: None,
        network: Arc::new(FakeNetwork::default()),
    });
    let state = host.start(&[shared.id().to_string()], 0).await.unwrap();
    // A Windows host runs another computer's Antigravity only with the switch on.
    host.set_allows_full_access(true);
    let (client_store, client_folder) = client_store();
    let seen = Arc::new(Collected::default());
    let client = client_with(&client_store, &client_folder, Arc::new(MemoryVault::default()), &seen, loopback_options());
    let ended = |seen: &Collected| {
        seen.events()
            .iter()
            .filter(|e| e.kind == "status" && matches!(e.status.as_deref(), Some("completed" | "error" | "stopped")))
            .count()
    };
    let info = client.connect("", &format!("127.0.0.1:{}", state.port.unwrap()), state.key.as_deref().unwrap()).await.unwrap();
    let workspace = client.open_workspace(&info.id, shared.id()).unwrap();
    let ask = |input: &str, resume: bool, grant: Option<AllowGrant>| {
        let mut request = StartRunRequest::new("pane", workspace.id(), "claude", input).with_provider("gemini");
        if resume {
            request.resume_id = Some(CONVERSATION.into());
        }
        request.antigravity_grant = grant;
        request
    };
    client.start(ask("say hi", false, None), &workspace).await.unwrap();
    until(|| ended(&seen) == 1, 20_000, "the first turn ends").await;
    let card = seen.events().iter().find_map(|e| e.antigravity_permission.clone()).expect("no card");
    assert_eq!(card.rules(), ["command(echo hi)"], "the host reported the card");

    // A rule the host never reported is refused, and nothing runs.
    let forged = AllowGrant { request_id: card.id.clone(), rules: vec!["command(rm -rf /)".into()], temporary: true };
    let refused = client.start(ask("go on", true, Some(forged)), &workspace).await.unwrap_err();
    assert!(refused.status() == Some(403) && refused.message() == get("remote.host.error.antigravityRules"), "a forged rule: {refused:?}");
    let unknown = AllowGrant { request_id: "agy-unknown".into(), rules: card.rules(), temporary: true };
    let refused = client.start(ask("go on", true, Some(unknown)), &workspace).await.unwrap_err();
    assert_eq!(refused.status(), Some(403), "a card the host never reported");
    assert!(text() == real && ended(&seen) == 1, "nothing changed and nothing ran");

    // The card's own rule: in place while the continuation runs, gone after.
    let once = continuation(&card, PermissionDecision::Once).unwrap();
    client.start(ask(&once.prompt, true, Some(once.grant.clone())), &workspace).await.unwrap();
    until(|| ended(&seen) == 2, 20_000, "the continuation ends").await;
    assert!(
        seen.events()
            .iter()
            .any(|e| e.entry.as_ref().is_some_and(|l| l.kind.as_deref() == Some("assistant") && l.text.as_deref() == Some("allowed"))),
        "the host's agy ran with the rule in place"
    );
    until(|| text() == real && rules.held().is_empty(), 10_000, "the rule is taken out").await;
    // A card's rules are granted once: the same grant again is refused.
    let refused = client.start(ask(&once.prompt, true, Some(once.grant.clone())), &workspace).await.unwrap_err();
    assert!(refused.status() == Some(403) && refused.message() == get("remote.host.error.antigravityRules"), "a used card");
    // Always is still refused for a rule that may be allowed only once.
    client.start(ask("risky", false, None), &workspace).await.unwrap();
    until(|| ended(&seen) == 3, 20_000, "the risky turn ends").await;
    let chained = seen.events().iter().rev().find_map(|e| e.antigravity_permission.clone()).unwrap();
    assert!(chained.rules() == ["command(echo hi && date)"] && !chained.can_allow_always(), "a chained command's card");
    let always = AllowGrant { request_id: chained.id.clone(), rules: chained.rules(), temporary: false };
    let refused = client.start(ask("go on", true, Some(always)), &workspace).await.unwrap_err();
    assert!(refused.status() == Some(409) && refused.message() == get("remote.error.antigravityOnceOnly"), "always for a once-only rule");
    assert_eq!(text(), real, "nothing was kept");
    client.dispose().await;
    host.dispose().await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    let _ = std::fs::remove_dir_all(&client_folder);
    let _ = std::fs::remove_dir_all(&root);
}
