//! The Rust half of `tools/parity`'s `styles project <manifest> <state> <out>` (Styles.cs): the panel bytes mighty-styles projects for
//! a manifest in a state, to be compared byte for byte with what MightyCode.Core's engine writes for the same arguments.

use std::collections::BTreeMap;

use mighty_core::locale::Locale;
use mighty_core::model::{BackgroundTask, BackgroundWork, TodoItem, TodoProgress};
use mighty_styles::manifest::{Vocabulary, items, text};
use mighty_styles::{RegisteredStyle, StylePrerequisiteResult, decoder, projection, state};
use serde_json::Value;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() != 4 || args[0] != "project" {
        eprintln!("usage: parity project <manifest> <state> <out>");
        std::process::exit(2);
    }
    let bytes = std::fs::read(&args[1]).expect("manifest");
    let s: Value = serde_json::from_slice(&std::fs::read(&args[2]).expect("state")).expect("state json");
    let text_of = |name: &str| s.get(name).and_then(Value::as_str).map(str::to_string);
    let list = |name: &str| -> Vec<String> {
        s.get(name).and_then(Value::as_array).map(|a| a.iter().map(|e| e.as_str().unwrap_or("").to_string()).collect()).unwrap_or_default()
    };
    let locale = Locale::new(&text_of("language").unwrap_or_else(|| "ko".into()));
    let vocabulary: Option<Vocabulary> = s.get("vocabulary").and_then(Value::as_object).map(|o| {
        o.iter()
            .map(|(k, v)| {
                (k.clone(), v.as_array().map(|a| a.iter().filter_map(|e| e.as_str().map(str::to_string)).collect()).unwrap_or_default())
            })
            .collect()
    });
    let source = text_of("source").unwrap_or_else(|| "user".into());
    let manifest = match decoder::decode(&bytes, &source, vocabulary.as_ref()) {
        Ok(m) => m,
        Err(e) => {
            std::fs::write(&args[3], format!("{}\n", e.code)).expect("out");
            return;
        }
    };
    let approval = if source == "bundled" { "preApproved" } else { "approved" };
    let style = RegisteredStyle::decoded(manifest, &source, &args[1], None, approval, &bytes);
    let reading = style
        .manifest
        .root
        .get("stateSources")
        .cloned()
        .filter(|_| s.get("checklist").is_some() || s.get("stage").is_some() || s.get("events").is_some())
        .map(|sources| {
            let checklist = text_of("checklist").unwrap_or_default();
            let files: Vec<_> = items(&sources, "files")
                .iter()
                .map(|f| state::parse(&checklist, text(f, "parser").unwrap_or_default(), text(f, "widget").unwrap_or_default(), &locale))
                .collect();
            let events: Vec<(String, String)> = s
                .get("events")
                .and_then(Value::as_array)
                .map(|a| a.iter().map(|p| (p[0].as_str().unwrap_or("").to_string(), p[1].as_str().unwrap_or("").to_string())).collect())
                .unwrap_or_default();
            let todos = TodoProgress {
                items: Some(
                    (1..=7)
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
                        .collect(),
                ),
                run_id: None,
            };
            let task = |id: &str, kind: &str, d: &str, at: &str, status: &str, ended: Option<&str>| BackgroundTask {
                id: Some(id.into()),
                kind: Some(kind.into()),
                description: Some(d.into()),
                started_at: Some(at.into()),
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
            match text_of("stage").filter(|_| style.evaluator().reads_plan_state()) {
                Some(stage) => state::reading(&style.manifest, &files, &events, Some(&stage), Some(&todos), Some(&work), &locale),
                None => state::reading(&style.manifest, &files, &events, None, None, None, &locale),
            }
        });
    let prerequisites = StylePrerequisiteResult {
        ready: s.get("ready").and_then(Value::as_bool).unwrap_or(true),
        missing: list("missing"),
        hint: text_of("hint"),
        install_command: None,
    };
    let group = text_of("group");
    let running = s.get("running").and_then(Value::as_bool).unwrap_or(false);
    let panel =
        projection::make(&style, &list("prompts"), group.as_deref(), &BTreeMap::new(), &prerequisites, running, None, reading.as_ref());
    std::fs::write(&args[3], projection::serialise(&panel)).expect("out");
}
