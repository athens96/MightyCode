//! MC-32 end to end: the scheduler's steps (`mighty_core::automation::plan` and `engine`) driven with a fake clock, each fire run
//! through the real run engine (`RunService`, the OS's process host) with the stand-in CLI. A daily 09:00 schedule fires on time
//! into a fresh pane, its record follows the run to its end with its cost and the pull request link in the pane's words; the app
//! "closes" for two days and, read back from its saved state, catches up once for the most recent missed fire; a fire over the daily
//! budget is a skipped record that starts nothing; and a retry fires the job again, its record pointing at the one it repeats.

mod common;

use std::collections::HashMap;
use std::sync::Arc;

use chrono::{DateTime, Duration, MappedLocalTime, NaiveDateTime, TimeZone, Utc};
use mighty_core::automation::engine::{self, Begin, JobRef};
use mighty_core::automation::plan::{self, Due, Slot, SlotState};
use mighty_core::automation::records::{History, RecordState};
use mighty_core::automation::recurrence::{Recurrence, Zone};
use mighty_core::automation::spec::CatchUp;
use mighty_core::model::{AppSnapshot, RunSession, Workspace};
use mighty_run::model::StartRunRequest;
use mighty_run::service::{RunService, RunServiceOptions};

/// UTC as the test's time zone.
struct Utc0;

impl Zone for Utc0 {
    fn resolve(&self, wall: NaiveDateTime) -> MappedLocalTime<DateTime<Utc>> {
        Utc.from_local_datetime(&wall)
    }
    fn wall(&self, at: DateTime<Utc>) -> NaiveDateTime {
        at.naive_utc()
    }
}

fn at(d: u32, h: u32, m: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 11, d, h, m, 0).unwrap()
}

/// What the app's budget check says (spent today plus the run's estimate passing the budget).
fn over_budget(spent: f64, budget: Option<f64>, estimate: f64) -> Option<String> {
    budget.filter(|b| spent + estimate > *b).map(|b| format!("over the daily budget (${spent:.2} of ${b:.2})"))
}

struct Harness {
    service: Arc<RunService>,
    workspace: String,
    record: std::path::PathBuf,
}

impl Harness {
    /// A fresh agent pane, as a fire makes one.
    fn pane(&self, id: &str) -> String {
        let pane = RunSession {
            id: Some(id.into()),
            workspace_id: Some(self.workspace.clone()),
            provider: Some("codex".into()),
            title: Some("deps".into()),
            ..RunSession::default()
        };
        self.service
            .update(|s| {
                let mut sessions = s.sessions().to_vec();
                sessions.push(Arc::new(pane.clone()));
                AppSnapshot { sessions: Some(sessions), ..s.clone() }
            })
            .unwrap();
        id.to_string()
    }

    /// Sends `prompt` to `pane` and waits for its run to end: the pane's status and its last words.
    async fn run(&self, pane: &str, prompt: &str) -> (String, String) {
        let request = StartRunRequest::new(pane, &self.workspace, "claude", prompt).with_provider("codex");
        self.service.start(request).await.unwrap();
        let ended = || {
            !self.service.is_session_running(pane)
                && self.service.snapshot().session(pane).and_then(|s| s.status.clone()).as_deref() != Some("running")
        };
        common::until(ended, 30000, "the fire's run ends").await;
        let session = self.service.snapshot().session(pane).unwrap().as_ref().clone();
        let words = session.logs().iter().map(|e| e.text().to_string()).collect::<Vec<_>>().join("\n");
        (session.status().to_string(), words)
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_schedule_fires_through_the_run_engine_catches_up_once_skips_over_budget_and_retries() {
    common::korean();
    let directory = common::temp("automation");
    let folder = directory.join("workspace");
    std::fs::create_dir_all(&folder).unwrap();
    let record = directory.join("fake");
    let catalog = common::fake_catalog(&["--fake-cli", "codex", &record.to_string_lossy()]);
    let (service, _events) = RunService::new(
        AppSnapshot::default(),
        catalog,
        RunServiceOptions {
            plugin_directory: String::new(),
            permission_card: true,
            remote: None,
            configure: None,
            admitting: None,
            binding: None,
            delegation: None,
        },
    );
    let workspace = Workspace { name: Some("app".into()), path: Some(folder.to_string_lossy().into_owned()), ..Workspace::default() };
    service
        .update(|s| AppSnapshot { workspaces: Some(vec![workspace.clone()]), active_workspace_id: workspace.id.clone(), ..s.clone() })
        .unwrap();
    let harness = Harness { service: Arc::new(service), workspace: workspace.id.clone().unwrap(), record };

    let job = JobRef {
        key: format!("{}:app:s:deps", harness.workspace),
        workspace: harness.workspace.clone(),
        workspace_name: "app".into(),
        name: "deps".into(),
        origin: "app".into(),
        kind: "schedule".into(),
    };
    let slots =
        vec![Slot { key: job.key.clone(), recurrence: Recurrence::cron("0 9 * * *").unwrap(), catch_up: CatchUp::Once, enabled: true }];
    let mut states: HashMap<String, SlotState> = HashMap::new();
    let mut history = History::default();
    let prompt = "Check the dependencies; open a PR. Last time: https://github.com/acme/app/pull/42";

    // Monday 08:00: nothing is due; the one timer sleeps until 09:00.
    let mut clock = at(23, 8, 0);
    assert!(plan::due(&slots, &mut states, clock, &Utc0).is_empty());
    let wake = plan::next_wake(&slots, &states, clock, &Utc0).unwrap();
    assert_eq!(wake, at(23, 9, 0));

    // It wakes a moment late: the schedule fires on time into a fresh pane, through the run engine.
    clock = wake + Duration::seconds(3);
    let due = plan::due(&slots, &mut states, clock, &Utc0);
    assert_eq!(due, vec![Due::Fire { key: job.key.clone(), at: wake, late: false }]);
    let fired = engine::begin(
        &mut history,
        &job,
        Begin { reason: "schedule".into(), scheduled_for: Some(wake), over_budget: over_budget(0.0, None, 1.0), ..Begin::default() },
        "r1".into(),
        clock,
    );
    assert_eq!(fired.state, RecordState::Running);
    let pane = harness.pane("auto-1");
    engine::attach(&mut history, "r1", &pane);
    let (status, words) = harness.run(&pane, prompt).await;
    assert_eq!(status, "completed");
    assert_eq!(std::fs::read_to_string(format!("{}.prompt", harness.record.display())).unwrap(), prompt, "the CLI got the prompt");
    assert!(words.contains("FAKE_CLI_OK"), "{words}");
    let done = engine::finish(&mut history, &pane, &status, Some(0.25), &words, clock).unwrap();
    assert_eq!((done.state, done.cost_usd), (RecordState::Done, Some(0.25)));
    assert_eq!(done.url.as_deref(), Some("https://github.com/acme/app/pull/42"));
    assert_eq!(done.pane.as_deref(), Some("auto-1"));

    // The app closes until Wednesday noon; its state is read back as the next start reads it. Tuesday's and Wednesday's 09:00 were
    // missed: only Wednesday's runs, once.
    let saved = serde_json::to_string(&states).unwrap();
    let mut states: HashMap<String, SlotState> = serde_json::from_str(&saved).unwrap();
    clock = at(25, 12, 0);
    let due = plan::due(&slots, &mut states, clock, &Utc0);
    assert_eq!(due, vec![Due::Fire { key: job.key.clone(), at: at(25, 9, 0), late: true }]);
    assert!(plan::due(&slots, &mut states, clock, &Utc0).is_empty(), "once");
    let caught = engine::begin(&mut history, &job, Begin { reason: "catch-up".into(), ..Begin::default() }, "r2".into(), clock);
    let pane = harness.pane("auto-2");
    engine::attach(&mut history, &caught.id, &pane);
    let (status, words) = harness.run(&pane, "Check the dependencies.").await;
    let caught = engine::finish(&mut history, &pane, &status, Some(0.5), &words, clock).unwrap();
    assert_eq!((caught.state, caught.url.clone()), (RecordState::Done, None));

    // Thursday: $9.50 of a $10 budget spent, a run costs about $1: the fire is a skipped record and no pane is made.
    clock = at(26, 9, 0) + Duration::seconds(1);
    assert_eq!(plan::due(&slots, &mut states, clock, &Utc0).len(), 1);
    let panes_before = harness.service.snapshot().sessions().len();
    let skipped = engine::begin(
        &mut history,
        &job,
        Begin { reason: "schedule".into(), over_budget: over_budget(9.5, Some(10.0), 1.0), ..Begin::default() },
        "r3".into(),
        clock,
    );
    assert_eq!(skipped.state, RecordState::Skipped);
    assert!(skipped.error.as_deref().unwrap().contains("budget"));
    assert_eq!(harness.service.snapshot().sessions().len(), panes_before);

    // 다시 실행 on the first record: the job fires again, the new record pointing at it.
    let runnable = vec![job.clone()];
    let (again, repeated) = engine::retry_target(&history, "r1", &runnable).unwrap();
    assert_eq!(again.key, job.key);
    let retried = engine::begin(
        &mut history,
        again,
        Begin { reason: "retry".into(), retry_of: Some(repeated.id.clone()), ..Begin::default() },
        "r4".into(),
        clock,
    );
    let pane = harness.pane("auto-3");
    engine::attach(&mut history, &retried.id, &pane);
    let (status, words) = harness.run(&pane, prompt).await;
    let retried = engine::finish(&mut history, &pane, &status, None, &words, clock).unwrap();
    assert_eq!((retried.state, retried.retry_of.as_deref()), (RecordState::Done, Some("r1")));

    // The history as the app keeps it: newest last, saved and read back.
    let file = directory.join("history.json");
    history.save(&file).unwrap();
    let back = History::load(&file);
    let states: Vec<(&str, RecordState)> = back.records.iter().map(|r| (r.id.as_str(), r.state)).collect();
    assert_eq!(states, vec![("r1", RecordState::Done), ("r2", RecordState::Done), ("r3", RecordState::Skipped), ("r4", RecordState::Done)]);
    assert_eq!(back.digest("2026-11-23T00:00:00Z").cost_usd, 0.75);

    harness.service.dispose().await;
    common::remove_when_free(&directory).await;
}
