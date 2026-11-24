//! MC-48: the verification gate — `verify` in `.mighty.yaml` (read, checked, merged, hashed), the loop that verifies, asks the agent
//! to repair and verifies again with its stops (a fake agent over real commands), the failure summary's redaction and limits, the
//! browser check's address rules, the DevTools findings and the receipt.

mod support;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use mighty_delegation::mighty_yaml::{self, VerifyWhen, load_from_folder, parse_config};
use mighty_delegation::run_actions;
use mighty_delegation::verify::{
    self, BrowserReport, BrowserScope, CommandResult, LoopSettings, PageFindings, RepairOutcome, RoundResult, StopReason, VerifyHost,
    VerifyReceipt, VerifyStatus,
};
use mighty_platform::SpawnRequest;
use serde_json::json;

// ── .mighty.yaml ────────────────────────────────────────────────────────────────────────────────────────────────────────

#[test]
fn verify_commands_rounds_when_and_browser_are_read() {
    let config = parse_config(
        "verify:\n  commands:\n    typecheck: npx tsc --noEmit\n    test:\n      command: npm test\n      cwd: web\n      timeoutSeconds: 600\n  maxRounds: 3\n  when: always\n  browser:\n    url: http://localhost:5173\n    paths: [/, /settings]\n",
    )
    .unwrap();
    let verify = &config.verify;
    assert_eq!(verify.commands.keys().collect::<Vec<_>>(), ["typecheck", "test"]);
    assert_eq!(verify.commands["typecheck"].timeout(), Duration::from_secs(300), "300 s when none is named");
    assert_eq!(verify.commands["test"].cwd.as_deref(), Some("web"));
    assert_eq!(verify.commands["test"].timeout(), Duration::from_secs(600));
    assert_eq!(verify.rounds(), 3);
    assert_eq!(verify.when(), VerifyWhen::Always);
    let browser = verify.browser.as_ref().unwrap();
    assert_eq!(browser.url.as_deref(), Some("http://localhost:5173"));
    assert_eq!(browser.visit_paths(), ["/", "/settings"]);

    // A list of named commands; the defaults.
    let listed = parse_config("verify:\n  commands:\n    - name: lint\n      command: npm run lint\n").unwrap();
    assert_eq!(listed.verify.commands["lint"].command, "npm run lint");
    assert_eq!(listed.verify.rounds(), mighty_yaml::DEFAULT_VERIFY_MAX_ROUNDS);
    assert_eq!(listed.verify.when(), VerifyWhen::Changed);
    assert!(listed.verify.browser.is_none());
    let run_action = parse_config("run:\n  web: npm run dev\nverify:\n  browser:\n    runAction: web\n").unwrap();
    assert_eq!(run_action.verify.browser.as_ref().unwrap().run_action.as_deref(), Some("web"));
    assert_eq!(run_action.verify.browser.as_ref().unwrap().visit_paths(), ["/"]);
}

#[test]
fn bad_verify_values_are_refused() {
    for (yaml, words) in [
        ("verify:\n  maxRounds: 6\n", "verify.maxRounds"),
        ("verify:\n  when: sometimes\n", "verify.when"),
        ("verify:\n  commands:\n    t:\n      command: x\n      timeoutSeconds: 0\n", "timeoutSeconds"),
        ("verify:\n  commands:\n    t:\n      cwd: ../outside\n      command: x\n", "cwd"),
        ("verify:\n  commands:\n    t:\n      description: x\n", "Unknown key"),
        ("verify:\n  commands:\n    t: \"\"\n", "cannot be empty"),
        ("verify:\n  browser:\n    url: https://example.com\n", "this computer"),
        ("verify:\n  browser:\n    url: http://localhost:3000\n    runAction: web\n", "only one"),
        ("verify:\n  browser:\n    paths: [relative]\n", "paths"),
        ("verify:\n  browser:\n    paths: [//evil.example]\n", "paths"),
        ("verify:\n  other: 1\n", "verify.other"),
        ("verify: npm test\n", "mapping"),
        ("verify:\n  commands:\n    - command: x\n", "name"),
    ] {
        let error = parse_config(yaml).unwrap_err();
        assert!(error.contains(words), "{yaml:?}: {error}");
    }
    let folder = support::place::temp("verify-yaml-action");
    std::fs::write(folder.join(".mighty.yaml"), "verify:\n  browser:\n    runAction: missing\n").unwrap();
    let error = load_from_folder(&folder).unwrap_err();
    assert!(error.contains("not under 'run'"), "{error}");
    support::place::remove_folder(&folder);
}

#[test]
fn verify_changes_the_hash_so_it_needs_approval_again_and_a_local_overlay_merges() {
    let folder = support::place::temp("verify-yaml-hash");
    std::fs::write(folder.join(".mighty.yaml"), "tasks:\n  test: npm test\n").unwrap();
    let before = load_from_folder(&folder).unwrap().unwrap();
    let json = serde_json::to_string(&before.config).unwrap();
    assert!(!json.contains("verify"), "a file without verify serializes as before: {json}");

    std::fs::write(folder.join(".mighty.yaml"), "tasks:\n  test: npm test\nverify:\n  commands:\n    test: npm test\n").unwrap();
    let with = load_from_folder(&folder).unwrap().unwrap();
    assert_ne!(before.content_hash, with.content_hash, "adding verify changes what was approved");

    std::fs::write(folder.join(".mighty.yaml"), "tasks:\n  test: npm test\nverify:\n  commands:\n    test: npm test -- --ci\n").unwrap();
    let changed = load_from_folder(&folder).unwrap().unwrap();
    assert_ne!(with.content_hash, changed.content_hash, "changing a verify command needs approval again");

    std::fs::write(folder.join(".mighty.local.yaml"), "verify:\n  commands:\n    lint: npm run lint\n  maxRounds: 0\n").unwrap();
    let merged = load_from_folder(&folder).unwrap().unwrap();
    assert_eq!(merged.config.verify.commands.keys().collect::<Vec<_>>(), ["test", "lint"]);
    assert_eq!(merged.config.verify.rounds(), 0);
    support::place::remove_folder(&folder);
}

// ── the loop over real commands with a fake agent ───────────────────────────────────────────────────────────────────────

fn shell(command: &str, folder: &Path) -> SpawnRequest {
    if cfg!(windows) {
        let comspec = std::env::var("ComSpec").unwrap_or_else(|_| r"C:\Windows\System32\cmd.exe".into());
        SpawnRequest::new(comspec, ["/d", "/s", "/c", command], folder)
    } else {
        SpawnRequest::new("/bin/sh", ["-c", command], folder)
    }
}

/// What the fake agent does when asked to repair.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Agent {
    /// Writes `fixed`, which the check looks for.
    Fixes,
    /// Changes nothing.
    Idle,
    /// Writes a different wrong answer each time (`attempt-<n>` in the output).
    Varies,
    /// The person typed into the pane while it worked.
    PersonSends,
    /// The turn ended badly.
    Fails,
}

struct FakeHost {
    folder: PathBuf,
    agent: Agent,
    budget_turns: u32,
    prompts: Vec<String>,
    updates: Vec<VerifyStatus>,
    person: bool,
}

impl FakeHost {
    fn new(name: &str, agent: Agent) -> FakeHost {
        FakeHost { folder: support::place::temp(name), agent, budget_turns: 99, prompts: vec![], updates: vec![], person: false }
    }

    fn check(&self) -> &'static str {
        if cfg!(windows) {
            "if exist fixed (echo all good) else (type attempt 2>nul & echo error TS2322: Type 'string' is not assignable at src/app.ts:12:5 token=ghp_abcdefghijklmnopqrstuvwxyz0123456789 & exit 1)"
        } else {
            "if [ -f fixed ]; then echo all good; else cat attempt 2>/dev/null; echo \"error TS2322: Type 'string' is not assignable at src/app.ts:12:5 token=ghp_abcdefghijklmnopqrstuvwxyz0123456789\"; exit 1; fi"
        }
    }
}

impl VerifyHost for FakeHost {
    async fn verify(&mut self) -> RoundResult {
        let processes = support::place::processes();
        let began = Instant::now();
        let outcome = run_actions::run_bounded(processes.as_ref(), shell(self.check(), &self.folder), Duration::from_secs(30)).await;
        let passed = outcome.succeeded();
        RoundResult {
            commands: vec![CommandResult {
                name: "typecheck".into(),
                command: "npx tsc --noEmit".into(),
                passed,
                exit_code: outcome.exit_code,
                timed_out: outcome.timed_out,
                duration_ms: began.elapsed().as_millis() as u64,
                output: outcome.output,
                error: outcome.error,
            }],
            browser: None,
        }
    }

    fn person_stepped_in(&mut self) -> bool {
        self.person
    }

    fn budget_allows(&mut self) -> bool {
        if self.budget_turns == 0 {
            return false;
        }
        self.budget_turns -= 1;
        true
    }

    async fn repair(&mut self, prompt: String) -> RepairOutcome {
        self.prompts.push(prompt);
        match self.agent {
            Agent::Fixes => {
                std::fs::write(self.folder.join("fixed"), "ok").unwrap();
                RepairOutcome::Done
            }
            Agent::Idle => RepairOutcome::Done,
            Agent::Varies => {
                std::fs::write(self.folder.join("attempt"), format!("attempt-{}\n", "x".repeat(self.prompts.len()))).unwrap();
                RepairOutcome::Done
            }
            Agent::PersonSends => {
                self.person = true;
                RepairOutcome::Done
            }
            Agent::Fails => RepairOutcome::Ended,
        }
    }

    fn update(&mut self, receipt: &VerifyReceipt) {
        self.updates.push(receipt.status);
    }
}

fn redact(text: &str) -> String {
    text.lines()
        .map(|line| if line.contains("ghp_") { "[redacted: GitHub token]".to_string() } else { line.to_string() })
        .collect::<Vec<_>>()
        .join("\n")
}

async fn run(host: &mut FakeHost, max_rounds: u32, auto_repair: bool) -> VerifyReceipt {
    let mut receipt = VerifyReceipt::new("r1", "abc123", max_rounds, "2026-11-24T00:00:00Z");
    let settings = LoopSettings { max_rounds, auto_repair, repair_header: "검증 실패 — 고쳐 주세요 ({round}/{max})".into() };
    verify::run_loop(host, &mut receipt, &settings, &redact).await;
    receipt
}

#[cfg(any(windows, target_os = "macos"))]
#[tokio::test(flavor = "multi_thread")]
async fn a_pass_the_first_time_records_a_receipt_and_asks_nothing() {
    let mut host = FakeHost::new("verify-pass", Agent::Idle);
    std::fs::write(host.folder.join("fixed"), "").unwrap();
    let receipt = run(&mut host, 2, true).await;
    assert_eq!(receipt.status, VerifyStatus::Passed);
    assert_eq!((receipt.rounds, receipt.repairs), (1, 0));
    assert!(host.prompts.is_empty());
    assert!(receipt.commands[0].passed && receipt.commands[0].output.contains("all good"));
    assert_eq!(host.updates.first(), Some(&VerifyStatus::Running));
    support::place::remove_folder(&host.folder);
}

#[cfg(any(windows, target_os = "macos"))]
#[tokio::test(flavor = "multi_thread")]
async fn a_failure_is_sent_back_repaired_and_verified_again() {
    let mut host = FakeHost::new("verify-repair", Agent::Fixes);
    let receipt = run(&mut host, 2, true).await;
    assert_eq!(receipt.status, VerifyStatus::Passed, "{receipt:?}");
    assert_eq!((receipt.rounds, receipt.repairs), (2, 1));
    assert_eq!(host.prompts.len(), 1);
    let prompt = &host.prompts[0];
    assert!(prompt.starts_with("검증 실패 — 고쳐 주세요 (1/2)"), "{prompt}");
    assert!(prompt.contains("typecheck") && prompt.contains("exit code 1"), "{prompt}");
    assert!(!prompt.contains("ghp_"), "the token never reaches the agent: {prompt}");
    assert!(host.updates.contains(&VerifyStatus::Repairing));
    support::place::remove_folder(&host.folder);
}

#[cfg(any(windows, target_os = "macos"))]
#[tokio::test(flavor = "multi_thread")]
async fn the_same_failure_twice_stops_early() {
    let mut host = FakeHost::new("verify-same", Agent::Idle);
    let receipt = run(&mut host, 5, true).await;
    assert_eq!(receipt.status, VerifyStatus::Failed);
    assert_eq!(receipt.stop_reason, Some(StopReason::SameFailure));
    assert_eq!((receipt.rounds, receipt.repairs), (2, 1), "one repair, then the same failure stops it");
    support::place::remove_folder(&host.folder);
}

#[cfg(any(windows, target_os = "macos"))]
#[tokio::test(flavor = "multi_thread")]
async fn max_rounds_bounds_the_repairs() {
    let mut host = FakeHost::new("verify-max", Agent::Varies);
    let receipt = run(&mut host, 2, true).await;
    assert_eq!(receipt.stop_reason, Some(StopReason::MaxRounds), "{receipt:?}");
    assert_eq!((receipt.rounds, receipt.repairs), (3, 2));
    assert_eq!(host.prompts.len(), 2);

    let mut zero = FakeHost::new("verify-zero", Agent::Fixes);
    let receipt = run(&mut zero, 0, true).await;
    assert_eq!(receipt.stop_reason, Some(StopReason::RepairOff), "maxRounds: 0 verifies and reports");
    assert!(zero.prompts.is_empty());
    support::place::remove_folder(&host.folder);
    support::place::remove_folder(&zero.folder);
}

#[cfg(any(windows, target_os = "macos"))]
#[tokio::test(flavor = "multi_thread")]
async fn the_budget_the_person_a_bad_turn_and_the_toggle_stop_it() {
    let mut budget = FakeHost::new("verify-budget", Agent::Varies);
    budget.budget_turns = 1;
    let receipt = run(&mut budget, 5, true).await;
    assert_eq!(receipt.stop_reason, Some(StopReason::Budget));
    assert_eq!(receipt.repairs, 1);

    let mut person = FakeHost::new("verify-person", Agent::PersonSends);
    let receipt = run(&mut person, 5, true).await;
    assert_eq!(receipt.stop_reason, Some(StopReason::Person));
    assert_eq!((receipt.rounds, receipt.repairs), (1, 1), "nothing runs after the person stepped in");

    let mut failed = FakeHost::new("verify-ended", Agent::Fails);
    let receipt = run(&mut failed, 5, true).await;
    assert_eq!(receipt.stop_reason, Some(StopReason::RepairEnded));

    let mut off = FakeHost::new("verify-off", Agent::Fixes);
    let receipt = run(&mut off, 3, false).await;
    assert_eq!(receipt.stop_reason, Some(StopReason::RepairOff));
    assert!(off.prompts.is_empty());
    for host in [budget, person, failed, off] {
        support::place::remove_folder(&host.folder);
    }
}

// ── the summary ─────────────────────────────────────────────────────────────────────────────────────────────────────────

fn failing(name: &str, output: String) -> CommandResult {
    CommandResult {
        name: name.into(),
        command: format!("run {name}"),
        passed: false,
        exit_code: Some(2),
        timed_out: false,
        duration_ms: 10,
        output,
        error: None,
    }
}

#[test]
fn the_summary_keeps_each_failures_end_redacted_and_within_the_limit() {
    let long: String = (0..5_000).map(|i| format!("line {i} of noise\n")).collect();
    let round = RoundResult {
        commands: vec![
            failing("test", format!("{long}secret=ghp_abcdefghijklmnopqrstuvwxyz0123456789\nFAILED the_last_test\n")),
            failing("lint", "\u{1b}[31merror\u{1b}[0m no-unused-vars\n".into()),
            CommandResult { passed: true, ..failing("build", "fine".into()) },
        ],
        browser: Some(BrowserReport {
            browser: Some("Chrome".into()),
            pages: vec![PageFindings {
                url: "http://localhost:3000/".into(),
                console_errors: vec!["Uncaught TypeError: x is undefined".into()],
                failed_requests: vec!["500 GET http://localhost:3000/api".into()],
                ..PageFindings::default()
            }],
            unavailable: None,
        }),
    };
    let summary = verify::failure_summary(&round, &redact);
    assert!(summary.chars().count() <= verify::MAXIMUM_SUMMARY_CHARS, "{}", summary.chars().count());
    assert!(summary.contains("FAILED the_last_test"), "the end of the output is kept");
    assert!(!summary.contains("line 10 of noise"), "the start of a long log is not");
    assert!(!summary.contains("ghp_") && summary.contains("[redacted"));
    assert!(summary.contains("### lint") && summary.contains("error no-unused-vars"), "ANSI codes go: {summary}");
    assert!(!summary.contains("### build"), "a passing command is not in it");
    assert!(summary.contains("500 GET http://localhost:3000/api") && summary.contains("x is undefined"));
    assert!(verify::failure_summary(&RoundResult::default(), &redact).is_empty());
}

#[test]
fn the_failure_hash_ignores_numbers_and_ids_but_not_new_errors() {
    let a = RoundResult { commands: vec![failing("test", "took 1.23s at 12:00:01 id deadbeef42\nerror: boom".into())], browser: None };
    let b = RoundResult { commands: vec![failing("test", "took 9.87s at 13:45:59 id cafebabe99\nerror: boom".into())], browser: None };
    let c = RoundResult { commands: vec![failing("test", "took 1.23s\nerror: other".into())], browser: None };
    assert_eq!(verify::failure_hash(&a), verify::failure_hash(&b));
    assert_ne!(verify::failure_hash(&a), verify::failure_hash(&c));
}

// ── the browser check ───────────────────────────────────────────────────────────────────────────────────────────────────

#[test]
fn only_this_worktrees_local_addresses_are_visited() {
    let scope = BrowserScope { block: Some((4100, 10)), detected: Some("http://localhost:5173/".into()) };
    for allowed in ["http://localhost:4100/", "http://127.0.0.1:4109/app", "http://localhost:5173/x", "http://[::1]:4105"] {
        assert!(verify::allowed_browser_url(allowed, &scope).is_ok(), "{allowed}");
    }
    for refused in [
        "http://localhost:4110/",
        "http://localhost:3000/",
        "https://example.com/",
        "http://192.168.0.2:4100/",
        "http://user:pw@localhost:4100/",
        "file:///etc/passwd",
        "http://localhost.evil.example:4100/",
        "http://localhost/",
    ] {
        assert!(verify::allowed_browser_url(refused, &scope).is_err(), "{refused}");
    }
    let targets = verify::browser_targets("http://localhost:4100/base/", &["/".into(), "/settings?x=1".into()], &scope).unwrap();
    assert_eq!(targets.iter().map(|u| u.as_str()).collect::<Vec<_>>(), ["http://localhost:4100/", "http://localhost:4100/settings?x=1"]);
    assert!(verify::browser_targets("http://localhost:4100/", &["//evil.example/".into()], &scope).is_err());
    assert!(verify::allowed_browser_url("http://localhost:4100/", &BrowserScope::default()).is_err(), "no block, no address: nothing");
}

#[test]
fn a_browser_is_found_at_its_known_places_only() {
    let mac = verify::browser_candidates("macos", Some(Path::new("/Users/me")), &[]);
    assert!(mac[0].1.ends_with("Google Chrome.app/Contents/MacOS/Google Chrome"));
    assert!(mac.iter().any(|(name, path)| name == "Brave" && path.starts_with("/Users/me/Applications")));
    let windows = verify::browser_candidates("windows", None, &[PathBuf::from(r"C:\Program Files")]);
    assert!(windows.iter().any(|(name, path)| name == "Microsoft Edge" && path.to_string_lossy().ends_with("msedge.exe")));
    let edge_only = verify::find_browser(&mac, |p| p.to_string_lossy().contains("Microsoft Edge"));
    assert_eq!(edge_only.map(|(name, _)| name), Some("Microsoft Edge".to_string()));
    assert!(verify::find_browser(&mac, |_| false).is_none(), "nothing installed: none, never a download");
}

#[test]
fn devtools_events_and_the_accessibility_tree_are_counted() {
    let mut page = PageFindings { url: "http://localhost:4100/".into(), ..PageFindings::default() };
    let mut requests = HashMap::new();
    let events = [
        (
            "Runtime.consoleAPICalled",
            json!({"type": "error", "args": [{"type": "string", "value": "Failed to load"}, {"type": "object", "description": "Error: boom"}]}),
        ),
        ("Runtime.consoleAPICalled", json!({"type": "log", "args": [{"type": "string", "value": "hello"}]})),
        (
            "Runtime.exceptionThrown",
            json!({"exceptionDetails": {"text": "Uncaught", "exception": {"description": "TypeError: x is undefined"}}}),
        ),
        ("Network.requestWillBeSent", json!({"requestId": "1", "request": {"method": "GET", "url": "http://localhost:4100/"}})),
        (
            "Network.responseReceived",
            json!({"requestId": "1", "type": "Document", "response": {"status": 200, "url": "http://localhost:4100/"}}),
        ),
        ("Network.requestWillBeSent", json!({"requestId": "2", "request": {"method": "POST", "url": "http://localhost:4100/api"}})),
        (
            "Network.responseReceived",
            json!({"requestId": "2", "type": "XHR", "response": {"status": 503, "url": "http://localhost:4100/api"}}),
        ),
        ("Network.requestWillBeSent", json!({"requestId": "3", "request": {"method": "GET", "url": "http://localhost:4100/font.woff"}})),
        ("Network.loadingFailed", json!({"requestId": "3", "errorText": "net::ERR_CONNECTION_REFUSED"})),
        ("Network.loadingFailed", json!({"requestId": "4", "errorText": "net::ERR_ABORTED", "canceled": true})),
        ("Log.entryAdded", json!({"entry": {"level": "error", "source": "network", "text": "Failed to load resource"}})),
    ];
    for (method, params) in events {
        page.observe(method, &params, &mut requests);
    }
    assert_eq!(page.console_errors, ["Failed to load Error: boom"]);
    assert_eq!(page.exceptions, ["TypeError: x is undefined"]);
    assert_eq!(
        page.failed_requests,
        ["503 POST http://localhost:4100/api", "net::ERR_CONNECTION_REFUSED GET http://localhost:4100/font.woff"]
    );
    assert_eq!(page.status, Some(200));

    let nodes = vec![
        json!({"nodeId": "1", "role": {"value": "RootWebArea"}, "name": {"value": "App"}}),
        json!({"nodeId": "2", "parentId": "1", "role": {"value": "button"}, "name": {"value": ""}}),
        json!({"nodeId": "3", "parentId": "1", "ignored": true, "role": {"value": "generic"}}),
        json!({"nodeId": "4", "parentId": "3", "role": {"value": "link"}, "name": {"value": "Home"}}),
    ];
    page.read_accessibility(&nodes);
    assert_eq!(page.accessibility_nodes, 3);
    assert_eq!(page.accessibility, ["RootWebArea \"App\"", "  button", "  link \"Home\""]);
    assert_eq!(page.unnamed_controls, ["button"]);

    let report = BrowserReport { browser: Some("Chrome".into()), pages: vec![page], unavailable: None };
    assert!(report.failed());
    let counts = report.counts();
    assert_eq!((counts.pages, counts.console_errors, counts.exceptions, counts.failed_requests, counts.unnamed_controls), (1, 1, 1, 2, 1));
    assert!(!BrowserReport::unavailable("no browser").failed(), "no browser is no failure");
}

#[test]
fn the_receipt_follows_the_rounds_and_serializes_for_the_page() {
    let mut receipt = VerifyReceipt::new("r", "abc", 2, "t0");
    let round = RoundResult { commands: vec![failing("test", "x".into())], browser: Some(BrowserReport::unavailable("none")) };
    receipt.record(&round);
    assert_eq!(receipt.rounds, 1);
    assert!(!receipt.finished());
    receipt.status = VerifyStatus::Failed;
    receipt.stop_reason = Some(StopReason::SameFailure);
    let value = serde_json::to_value(&receipt).unwrap();
    assert_eq!(value["status"], "failed");
    assert_eq!(value["stopReason"], "sameFailure");
    assert_eq!(value["maxRounds"], 2);
    assert_eq!(value["commands"][0]["exitCode"], 2);
    assert_eq!(value["browser"]["unavailable"], "none");
    assert_eq!(value["browserCounts"]["pages"], 0);
    assert!(receipt.finished());
}
