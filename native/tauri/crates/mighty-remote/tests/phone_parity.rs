//! Parity with C# itself (W8-B, plan W8-B "Parity"): C#'s `MobileDesktopRouter` over a `DesktopService` loaded from a profile, no runs,
//! and this crate's router over the headless desk loaded from the same profile, both with the same window extras (queued rows, settings,
//! the Mighty payload with the pane's runs, a status line, an ask target, background work), answering the same GETs (`/m1/info`, `/m1/state`, each
//! pane's detail and entries, each workspace's files), compared as JSON values, then byte for byte. The profiles are the 9
//! `scripts/fixtures/delegation-record/*/profile` states, their `@{root}` placed in a folder of the check's own. Without the .NET 10 SDK
//! the check says so and passes; `MIGHTY_PARITY=require` makes it fail instead.

mod support;

use std::path::Path;
use std::process::Command;

use serde_json::Value;
use support::desk::{Temp, World};

#[allow(clippy::disallowed_methods, reason = "the check's own .NET probe, no child of the app")]
fn dotnet_10() -> bool {
    let listed = Command::new("dotnet").arg("--list-sdks").output();
    let found = matches!(&listed, Ok(output) if output.status.success() && String::from_utf8_lossy(&output.stdout).lines().any(|line| line.starts_with("10.")));
    if !found {
        let why = "no .NET 10 SDK (dotnet --list-sdks)";
        assert!(std::env::var("MIGHTY_PARITY").as_deref() != Ok("require"), "{why}");
        eprintln!("skipped: {why}");
    }
    found
}

/// `parity routes <profile> <requests> <out>` from tools/parity, built once.
#[allow(clippy::disallowed_methods, reason = "the check's own .NET build and run, no child of the app")]
fn csharp(profile: &Path, requests: &Path, output: &Path) {
    let tauri = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let out = tauri.join("target/parity-cs-phone");
    static BUILT: std::sync::Mutex<bool> = std::sync::Mutex::new(false);
    let mut built = BUILT.lock().unwrap_or_else(|p| p.into_inner());
    if !*built {
        let status = Command::new("dotnet")
            .arg("build")
            .arg(tauri.join("tools/parity/Parity.csproj"))
            .args(["-c", "Release", "-o"])
            .arg(&out)
            .args(["--nologo", "-v", "quiet"])
            .status();
        assert!(matches!(status, Ok(status) if status.success()), "the C# parity tool did not build ({status:?})");
        *built = true;
    }
    let ran = Command::new("dotnet").arg(out.join("parity.dll")).arg("routes").arg(profile).arg(requests).arg(output).status().unwrap();
    assert!(ran.success(), "the C# parity tool failed (routes)");
}

/// The profile copied into `temp/profile` with its `@{root}` placed in `temp/root` (its `repo` folder made).
fn place(source: &Path, temp: &Path) -> std::path::PathBuf {
    let root = temp.join("root");
    std::fs::create_dir_all(root.join("repo")).unwrap();
    std::fs::write(root.join("repo").join("README.md"), "# repo\n").unwrap();
    let profile = temp.join("profile");
    std::fs::create_dir_all(&profile).unwrap();
    // Forward slashes: a path JSON needs no escapes for, and both apps read as the same folder.
    let placed = root.to_string_lossy().replace('\\', "/");
    for entry in std::fs::read_dir(source).unwrap().flatten() {
        let mut text = std::fs::read_to_string(entry.path()).unwrap().replace("@{root}", &placed);
        // A pane saved without its creation time gets the load's clock on both sides: it is given one, so the two loads agree.
        if entry.file_name() == "workspace-state.json" {
            let mut state: Value = serde_json::from_str(&text).unwrap();
            for pane in state.get_mut("sessions").and_then(Value::as_array_mut).into_iter().flatten() {
                if let Some(pane) = pane.as_object_mut() {
                    pane.entry("createdAt").or_insert_with(|| Value::String("2026-10-10T00:00:00Z".into()));
                }
            }
            text = state.to_string();
        }
        std::fs::write(profile.join(entry.file_name()), text).unwrap();
    }
    profile
}

/// The state a `DesktopService` loads from the profile, as the app's store loads it.
fn loaded(profile: &Path) -> mighty_core::model::AppSnapshot {
    let mut store = mighty_core::store::Store::new(profile.to_path_buf(), None, false);
    store.load(&mighty_run::strings::locale()).expect("the profile loads")
}

/// What a profile's load makes on each side at its own time — a human child's pane delegation creates, the line a provider's migration
/// writes — carries that load's clock and a fresh id: in the JSON compared, a time of today reads `<now>` and such an entry's id `<id>`.
/// A reply holding none is compared byte for byte too.
fn scrub(value: &mut Value, today: &str) {
    match value {
        Value::Object(map) => {
            let fresh = map.get("timestamp").and_then(Value::as_str).is_some_and(|t| t.starts_with(today));
            if fresh {
                map.insert("id".into(), Value::String("<id>".into()));
            }
            for (key, item) in map.iter_mut() {
                if matches!(key.as_str(), "timestamp" | "updatedAt") && item.as_str().is_some_and(|t| t.starts_with(today)) {
                    *item = Value::String("<now>".into());
                } else {
                    scrub(item, today);
                }
            }
        }
        Value::Array(items) => items.iter_mut().for_each(|item| scrub(item, today)),
        _ => {}
    }
}

/// The extras Phone.cs's `Extras` gives, made from the same pane: queued rows, the settings record, the Mighty payload with the pane's runs,
/// a status line, an ask target, the background work and done count.
fn extras(pane: &mighty_core::model::RunSession) -> mighty_remote::phone::desk::MobilePaneExtras {
    use mighty_remote::phone::desk::{MobileAskTarget, MobileBackgroundWork, MobilePaneExtras, MobileQueuedItem};
    use mighty_remote::phone::mighty::{self, MobileOut};
    let l = mighty_run::strings::locale();
    let row = |id: &str, text: &str, run_now: Option<&str>, reason: Option<&str>| MobileQueuedItem {
        id: id.into(),
        text: text.into(),
        run_now: run_now.map(str::to_string),
        run_now_reason: reason.map(str::to_string),
    };
    let settings = mighty::MobileSettings {
        editable: true,
        model: pane.model().into(),
        permission_mode: pane.settings.clone().unwrap_or_default().permission_mode,
        effort: None,
        agent_view_mode: "mighty".into(),
        mighty_style: "cli".into(),
        style_id: "cli".into(),
        options: mighty::MobileSettingsOptions {
            models: vec![mighty::MobileOption::new("default", "Default")],
            permission_modes: vec![mighty::MobileOption::new("manual", "Manual")],
            efforts: None,
            mighty_styles: vec![mighty::MobileOption::new("cli", "CLI")],
            styles: vec![mighty::MobileStyleOption { id: "cli".into(), label: "cli".into(), source: None }],
        },
    };
    let runs = mighty::runs(&mighty::saved_runs(pane), &mighty::RunsInputs::default(), &l);
    let newest = mighty_graph::support::newest_request_run(pane.graph_runs());
    MobilePaneExtras {
        queued: vec![row("q-1", "Next request", Some("here"), None), row("q-2", "Later 한글", Some("unavailable"), Some("turn"))],
        settings: serde_json::from_str(&settings.to_json()).ok(),
        mighty: Some(
            serde_json::json!({"style": "cli", "styleId": "cli", "runs": serde_json::from_str::<Value>(&runs.to_json()).unwrap()}),
        ),
        status_line: Some(serde_json::json!({"lines": [[{"text": "main <branch>", "fg": "#FFAA00", "bold": true}]]})),
        ask_targets: Some(vec![MobileAskTarget { id: "codex".into(), label: "Codex".into() }]),
        ask_target: Some("codex".into()),
        background_work: mighty::mobile_background_work(pane.background_work.as_ref(), newest, None, &l)
            .map(|rows| rows.into_iter().map(MobileBackgroundWork::from).collect()),
        background_done: mighty::mobile_background_done(pane.background_work.as_ref()),
        terminal_prompt: None,
    }
}

#[tokio::test]
async fn phone_routes_answer_what_csharp_answers() {
    if !dotnet_10() {
        return;
    }
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../../scripts/fixtures/delegation-record");
    let mut cases: Vec<_> =
        std::fs::read_dir(&fixtures).unwrap().flatten().map(|e| e.path()).filter(|p| p.join("profile").is_dir()).collect();
    cases.sort();
    assert!(cases.len() >= 9, "the delegation-record profiles: {}", cases.len());
    for case in cases {
        let temp = Temp::new("phone-parity");
        let profile = place(&case.join("profile"), temp.path());
        let snapshot = loaded(&profile);
        let mut requests = vec!["/m1/info".to_string(), "/m1/state".to_string(), "/m1/nothing".to_string()];
        for pane in snapshot.sessions() {
            requests.push(format!("/m1/sessions/{}", pane.id()));
            requests.push(format!("/m1/sessions/{}/entries?limit=5", pane.id()));
        }
        for workspace in snapshot.workspaces() {
            requests.push(format!("/m1/workspaces/{}/files", workspace.id()));
        }
        let list = temp.path().join("requests.txt");
        std::fs::write(&list, requests.join("\n") + "\n").unwrap();
        let answers = temp.path().join("csharp.txt");
        csharp(&profile, &list, &answers);
        let expected = std::fs::read_to_string(&answers).unwrap();
        let world = World::over(Temp::new("phone-parity-desk"), snapshot, "host-parity", "Parity", |o| o.screen_share = false);
        world.desk.calls().extras = Some(std::sync::Arc::new(extras));
        for (path, line) in requests.iter().zip(expected.lines()) {
            let (status, body) = line.split_once('\t').expect("status and body");
            let reply = world.raw("GET", path, None).await;
            let today = &mighty_core::time::now_o()[..10];
            let mut want: Value = serde_json::from_str(body).unwrap();
            let mut got: Value = serde_json::from_str(&reply.body).unwrap();
            scrub(&mut want, today);
            scrub(&mut got, today);
            assert_eq!(reply.status.to_string(), status, "{}: {path} status", case.display());
            assert_eq!(got, want, "{}: {path} as JSON", case.display());
            if !body.contains(today) {
                assert_eq!(reply.body, body, "{}: {path} byte for byte", case.display());
            }
        }
    }
}
