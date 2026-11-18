//! The perf scenario (C# `WinUI\MainWindow.PerfScenario.cs`; `scripts\perf-scenario.ps1 -Engine tauri`; plan §1.8 "Perf scenario",
//! W9-H). Owner: W9-H.
//!
//! `--smoke-test --perf-scenario [--smoke-exit] --profile <empty>` runs in the smoke's place: the fixture providers (no CLI is read; runs.rs
//! finds none in this run), a few long conversations (copied from `MIGHTY_PERF_STATE`'s `workspace-state.json`, read from a copy, or made
//! up: four Claude panes of 300 entries and 15 finished requests each), and the operations people find slow, each timed in the page from
//! the action until the animation frame after the DOM shows its result (`__perf`, C#'s page helper as it is), while a 16 ms probe records
//! how late the app's UI thread answers. The phases: `switchTabsMighty` (20 tab clicks), `addPane` (five panes from the add menu, then
//! closed), `streamMighty` (an 8 s answer at 25 events a second through the run service's own event path while the composer is typed
//! into every 500 ms) and `sendBlockMighty` (Enter in the composer until the page draws the sent request, ten times; target one frame,
//! 50 ms). Each phase adds the page's long tasks. Nothing is sent to an AI: a send starts this exe's canned CLI (`--help-demo-cli`), which
//! answers the request at once.
//!
//! The result is `perf-result.json` in the profile with C#'s keys in C#'s order, beside `perf.log` (the phase marks and the scenario's own
//! timed host steps, read back into `perfLogSteps`) and `startup.log`. What only C#'s WinUI host has keeps its key with the Rust host's
//! true value: `fullRenders`, `tabSwitchesInPlace` and `paneAddsInPlace` are 0 (the page draws every layout; the host rebuilds none), and
//! `gc` is all zeros (no garbage collector).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use mighty_core::graph::MightyGraphRun;
use mighty_core::model::{AgentActivity, LogEntry, RunSession};
use mighty_core::wire;
use mighty_run::RunEvent;
use mighty_run::model::CliCommand;
use serde_json::{Map, Value, json};
use tauri::Manager;

use crate::help_capture::{js, literal};
use crate::host::{Host, WINDOW};

/// The longest an operation is waited for; one that reaches it is counted in the phase's `capped`, not taken as its time.
const SETTLE_CAP_MS: f64 = 30_000.0;
const IDLE_MS: u64 = 2000;
const ROUNDS: usize = 20;
const ADDED_PANES: usize = 5;
const STREAM_SECONDS: u64 = 8;
const EVENTS_PER_SECOND: u64 = 25;
const TYPING_EVERY_MS: f64 = 500.0;
const SENDS: usize = 10;

/// The page's half of the scenario, installed once (C# `PerfPageHelper`, as it is).
const PAGE_HELPER: &str = "(() => { if (window.__perf) return true; const p = window.__perf = { longTasks: [], observing: false, frame: () => new Promise(r => requestAnimationFrame(() => r())), \
slot: () => document.querySelector('.pane-slot.active:not(.hidden)'), \
async until(cond, t0) { while (!cond()) { if (performance.now() - t0 > 29900) throw new Error('capped'); await p.frame(); } }, \
start(body) { window.__perfOp = null; const t0 = performance.now(); (async () => { try { await body(t0); await p.frame(); window.__perfOp = { ms: performance.now() - t0 }; } \
catch (e) { window.__perfOp = { ms: performance.now() - t0, error: String((e && e.message) || e) }; } })(); return true; } }; \
try { new PerformanceObserver(list => { for (const e of list.getEntries()) p.longTasks.push(e.duration); }).observe({ type: 'longtask' }); p.observing = true; } catch { } return true; })()";

/// The page's long tasks since the phase began (C# `PerfPageLongTasks`).
const PAGE_LONG_TASKS: &str = "JSON.stringify((() => { const d = window.__perf?.longTasks ?? []; const r = x => Math.round(x * 10) / 10; \
return { observed: !!window.__perf?.observing, count: d.length, sumMs: r(d.reduce((a, b) => a + b, 0)), maxMs: r(Math.max(0, ...d)), over100: d.filter(x => x > 100).length, over250: d.filter(x => x > 250).length }; })())";

/// Called by the smoke once the page is up (smoke.rs) in a `--perf-scenario` run: the scenario takes over from the smoke's probe.
pub fn begin(host: &Host) -> bool {
    let Some(shared) = host.app.try_state::<Arc<Host>>().map(|h| h.inner().clone()) else { return false };
    host.log.line("perf scenario");
    let task = tauri::async_runtime::spawn(async move { run(shared).await });
    let watcher = host.app.try_state::<Arc<Host>>().map(|h| h.inner().clone());
    tauri::async_runtime::spawn(async move {
        // A panic ends the run as failed, with what it said in the log (the task alone would end silently and the app wait forever).
        if let (Err(error), Some(host)) = (task.await, watcher) {
            host.log.line(&format!("perf scenario: the run panicked: {error}"));
            if let Some(smoke) = &host.smoke {
                smoke.end(&host, false);
            }
        }
    });
    true
}

// ── perf.log and startup.log ──

/// C# `PerfLog`: each mark and each timed step appended to the profile's `perf.log` with the time of day.
struct PerfLog(PathBuf);

impl PerfLog {
    fn line(&self, text: &str) {
        use std::io::Write as _;
        let line = format!("{} {text}\n", chrono::Local::now().format("%H:%M:%S%.3f"));
        if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(&self.0) {
            let _ = file.write_all(line.as_bytes());
        }
    }

    /// C# `PerfLog.Mark`.
    fn mark(&self, what: &str) {
        self.line(&format!("    mark    {what}"));
    }

    /// C# `PerfLog.Measure`: a step written when it took 16 ms or more.
    fn step(&self, what: &str, started: Instant) {
        let ms = started.elapsed().as_secs_f64() * 1000.0;
        if ms >= 16.0 {
            self.line(&format!("{ms:8.1} ms {what}"));
        }
    }
}

/// C# `TraceStartup`: one stage in the profile's `startup.log`.
fn trace(directory: &Path, stage: &str) {
    use std::io::Write as _;
    if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(directory.join("startup.log")) {
        let _ = writeln!(file, "{} {stage}", mighty_core::time::now_o());
    }
}

/// One timed step's name and its count, total and longest milliseconds.
type Step = (String, (u64, f64, f64));

/// C# `PerfLogSteps`: perf.log's timed steps added up per scenario phase (the "perf phase" marks) and for the whole run, each phase's
/// steps by their total, the largest first.
pub fn log_steps(log: &str) -> Map<String, Value> {
    let mut totals: Vec<(String, Vec<Step>)> = vec![];
    let mut phase = "setup".to_string();
    for line in log.lines() {
        let mut words = line.splitn(2, char::is_whitespace);
        let (Some(_time), Some(rest)) = (words.next(), words.next()) else { continue };
        let rest = rest.trim_start();
        if let Some(mark) = rest.strip_prefix("mark") {
            let mark = mark.trim();
            if let Some(name) = mark.strip_prefix("perf phase ") {
                phase = name.trim().to_string();
            } else if mark == "perf idle" {
                phase = "idle".into();
            }
            continue;
        }
        let Some((number, what)) = rest.split_once(" ms ") else { continue };
        let Ok(ms) = number.trim().replace(',', ".").parse::<f64>() else { continue };
        let what = what.trim().to_string();
        for bucket in [phase.clone(), "all".to_string()] {
            let at = match totals.iter().position(|(name, _)| *name == bucket) {
                Some(at) => at,
                None => {
                    totals.push((bucket, vec![]));
                    totals.len() - 1
                }
            };
            let steps = &mut totals[at].1;
            match steps.iter_mut().find(|(name, _)| *name == what) {
                Some((_, (count, sum, max))) => {
                    *count += 1;
                    *sum += ms;
                    *max = max.max(ms);
                }
                None => steps.push((what.clone(), (1, ms, ms))),
            }
        }
    }
    let round = |x: f64| (x * 10.0).round() / 10.0;
    totals
        .into_iter()
        .map(|(bucket, mut steps)| {
            steps.sort_by(|a, b| b.1.1.total_cmp(&a.1.1));
            let steps: Map<String, Value> = steps
                .into_iter()
                .map(|(what, (count, sum, max))| (what, json!({"count": count, "totalMs": round(sum), "maxMs": round(max)})))
                .collect();
            (bucket, Value::Object(steps))
        })
        .collect()
}

// ── the stall probe ──

/// How late the UI thread ran a 16 ms tick during one phase (C# `PerfStalls`): a late tick is time nothing else could run either.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Stalls {
    ticks: u64,
    over100: u64,
    over250: u64,
    beyond_sum: f64,
    max: f64,
}

impl Stalls {
    pub fn add(&mut self, late: f64) {
        self.ticks += 1;
        self.max = self.max.max(late);
        if late > 33.0 {
            self.beyond_sum += late - 33.0;
        }
        if late > 100.0 {
            self.over100 += 1;
        }
        if late > 250.0 {
            self.over250 += 1;
        }
    }

    pub fn json(&self) -> Value {
        let round = |x: f64| (x * 10.0).round() / 10.0;
        json!({"ticks": self.ticks, "lateBeyond33SumMs": round(self.beyond_sum), "maxLateMs": round(self.max), "over100": self.over100, "over250": self.over250})
    }
}

/// The probe: a tick asked of the app's UI thread (Tauri's main thread) 16 ms after the last one ran; each tick's lateness is the time
/// since the last one less the 16 ms asked for. The system timer's 15.6 ms step makes an idle tick some 15 ms late, which is why only
/// lateness beyond 33 ms is summed (C#'s note, the same here).
struct Probe {
    stalls: Arc<Mutex<Option<Stalls>>>,
    stop: Arc<AtomicBool>,
}

impl Probe {
    fn start(host: &Host) -> Probe {
        let stalls: Arc<Mutex<Option<Stalls>>> = Arc::default();
        let stop = Arc::new(AtomicBool::new(false));
        let (app, kept, stopping) = (host.app.clone(), stalls.clone(), stop.clone());
        let _ = std::thread::Builder::new().name("perf-probe".into()).spawn(move || {
            let mut last = Instant::now();
            while !stopping.load(Ordering::SeqCst) {
                std::thread::sleep(Duration::from_millis(16));
                let (sender, ran) = std::sync::mpsc::channel();
                if app
                    .run_on_main_thread(move || {
                        let _ = sender.send(Instant::now());
                    })
                    .is_err()
                {
                    break;
                }
                let Ok(at) = ran.recv_timeout(Duration::from_secs(30)) else { continue };
                let late = at.duration_since(last).as_secs_f64() * 1000.0 - 16.0;
                last = at;
                if let Some(stalls) = kept.lock().unwrap_or_else(|p| p.into_inner()).as_mut() {
                    stalls.add(late);
                }
            }
        });
        Probe { stalls, stop }
    }

    fn begin(&self) {
        *self.stalls.lock().unwrap_or_else(|p| p.into_inner()) = Some(Stalls::default());
    }

    fn take(&self) -> Stalls {
        self.stalls.lock().unwrap_or_else(|p| p.into_inner()).take().unwrap_or_default()
    }
}

impl Drop for Probe {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

/// C# `SettledAsync`: the UI has settled — everything queued on the UI thread before has run (a tick asked of it came back), the page
/// was sent the state, and a frame's time passed. A UI thread that does not answer within the cap ends the wait.
async fn settled(host: &Host) {
    host.flush();
    let (sender, ran) = tokio::sync::oneshot::channel();
    if host
        .app
        .run_on_main_thread(move || {
            let _ = sender.send(());
        })
        .is_ok()
    {
        let _ = tokio::time::timeout(Duration::from_millis(SETTLE_CAP_MS as u64), ran).await;
    }
    tokio::time::sleep(Duration::from_millis(16)).await;
}

// ── the result (each part built here, in C#'s key order; the checks build one the same way and compare it with C#'s) ──

/// C# `RunPerfScenario`'s first keys.
fn result_skeleton(started_at: &str) -> Map<String, Value> {
    let mut result = Map::new();
    result.insert("completed".into(), json!(false));
    result.insert("startedAt".into(), json!(started_at));
    result.insert("uiMode".into(), json!("web"));
    result.insert("aiRequestSent".into(), json!(false));
    result.insert("phases".into(), json!({}));
    result.insert("errors".into(), json!([]));
    result
}

/// C# `SeedPerfScenario`'s facts: what came from `MIGHTY_PERF_STATE` (its folder, workspaces and panes) and the panes made up.
fn seed_facts(imported: Option<(&str, usize, usize)>, synthesized: usize) -> Map<String, Value> {
    let mut facts = Map::new();
    if let Some((source, workspaces, panes)) = imported {
        facts.insert("importedFrom".into(), json!(source));
        facts.insert("importedWorkspaces".into(), json!(workspaces));
        facts.insert("importedPanes".into(), json!(panes));
    }
    facts.insert("synthesizedPanes".into(), json!(synthesized));
    facts
}

/// What the page drew for the scenario, before the first phase.
struct PageFacts {
    panes: usize,
    tabs: usize,
    entries: usize,
    active_entries: usize,
    graph_runs: usize,
    window: String,
}

fn add_page_facts(result: &mut Map<String, Value>, facts: &PageFacts) {
    result.insert("paneCount".into(), json!(facts.panes));
    result.insert("tabCount".into(), json!(facts.tabs));
    result.insert("totalLogEntries".into(), json!(facts.entries));
    result.insert("activePaneLogEntries".into(), json!(facts.active_entries));
    result.insert("graphRuns".into(), json!(facts.graph_runs));
    result.insert("windowSize".into(), json!(facts.window));
}

/// C#'s `Phase` result: the ops ranked, the wall time, the stalls, what the body added, the page's long tasks and the host's paths.
fn phase_json(ops: &[f64], wall_ms: f64, stall: &Stalls, body: Map<String, Value>, long_tasks: Value) -> Value {
    let mut sorted = ops.to_vec();
    sorted.sort_by(f64::total_cmp);
    let mut phase = Map::new();
    phase.insert("ops".into(), json!(ops));
    phase.insert("opMedian".into(), json!(rank(&sorted, 0.5)));
    phase.insert("opP95".into(), json!(rank(&sorted, 0.95)));
    phase.insert("opMax".into(), json!(sorted.last().copied().unwrap_or(0.0)));
    phase.insert("wallMs".into(), json!(wall_ms.round()));
    phase.insert("stall".into(), stall.json());
    phase.extend(body);
    phase.insert("pageLongTasks".into(), long_tasks);
    // Which paths the phase took: the page draws every layout, so the host rebuilds none in place or whole.
    phase.insert("fullRenders".into(), json!(0));
    phase.insert("tabSwitchesInPlace".into(), json!(0));
    phase.insert("paneAddsInPlace".into(), json!(0));
    phase.insert("capped".into(), json!(ops.iter().filter(|ms| **ms >= SETTLE_CAP_MS - 50.0).count()));
    // No garbage collector: nothing allocated through one, no collection, no pause.
    phase.insert("gc".into(), json!({"allocatedMB": 0.0, "gen0": 0, "gen1": 0, "gen2": 0, "pauseMs": 0.0}));
    Value::Object(phase)
}

/// What the addPane phase adds: the closes' times and how many panes it added.
fn add_pane_extra(closes: &[f64], added: usize) -> Map<String, Value> {
    let mut extra = Map::new();
    extra.insert("closeOps".into(), json!(closes));
    extra.insert("added".into(), json!(added));
    extra
}

/// What the stream phase adds: the events sent, those later than one gap, and the stream's length.
fn stream_extra(events: u64, late: u64, stream_ms: f64) -> Map<String, Value> {
    let mut extra = Map::new();
    extra.insert("events".into(), json!(events));
    extra.insert("eventsLateOverOneGap".into(), json!(late));
    extra.insert("streamMs".into(), json!(stream_ms.round()));
    extra
}

/// What the send phase adds: how many sends and the target.
fn send_extra() -> Map<String, Value> {
    let mut extra = Map::new();
    extra.insert("sends".into(), json!(SENDS));
    extra.insert("targetMs".into(), json!(50));
    extra
}

/// The last keys, once every phase ran.
fn finish_result(result: &mut Map<String, Value>, log_path: &Path, log: &str, finished_at: &str) {
    result.insert("perfLog".into(), json!(log_path.to_string_lossy()));
    result.insert("perfLogSteps".into(), Value::Object(log_steps(log)));
    result.insert("completed".into(), json!(true));
    result.insert("finishedAt".into(), json!(finished_at));
}

// ── the scenario ──

/// The scenario's state while it runs.
struct Scenario {
    host: Arc<Host>,
    log: PerfLog,
    probe: Probe,
    errors: Arc<Mutex<Vec<String>>>,
}

/// The phases' rank of a share of the sorted ops (C#'s `Rank`).
pub fn rank(sorted: &[f64], share: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let at = ((share * sorted.len() as f64).ceil() as i64 - 1).clamp(0, sorted.len() as i64 - 1) as usize;
    sorted[at]
}

fn round1(x: f64) -> f64 {
    (x * 10.0).round() / 10.0
}

impl Scenario {
    fn error(&self, text: String) {
        self.errors.lock().unwrap_or_else(|p| p.into_inner()).push(text);
    }

    /// C# `PageOp`: one operation in the page; the body acts on the DOM and awaits `__perf.until(...)` for its result, and the page's
    /// time is the op's. A failed or capped op counts as the cap, and its error is listed.
    async fn page_op(&self, ops: &mut Vec<f64>, body: &str) -> Result<f64, String> {
        self.host.view().error = None;
        if js(&self.host, &format!("window.__perf.start(async (t0) => {{ {body} }})")).await? != "true" {
            return Err("The page's perf helper is missing.".into());
        }
        let waited = Instant::now();
        let mut op = Value::Null;
        while waited.elapsed().as_secs_f64() * 1000.0 < SETTLE_CAP_MS + 1000.0 {
            let text: String = serde_json::from_str(&js(&self.host, "JSON.stringify(window.__perfOp)").await?).unwrap_or_default();
            op = serde_json::from_str(&text).unwrap_or(Value::Null);
            if op.is_object() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(15)).await;
        }
        let mut ms = op.get("ms").and_then(Value::as_f64).map_or(SETTLE_CAP_MS, round1);
        if let Some(failed) = op.get("error") {
            self.error(format!("page: {}", failed.as_str().unwrap_or("")));
            ms = ms.max(SETTLE_CAP_MS);
        }
        ops.push(ms);
        let banner = self.host.view().error.clone().filter(|e| !e.is_empty());
        if let Some(banner) = banner {
            self.error(banner);
        }
        Ok(ms)
    }

    /// C#'s `Phase`: two idle seconds, then the body timed with the stall probe on; the phase's ops ranked, its long tasks and the host's
    /// paths added.
    async fn phase(
        &self,
        name: &str,
        body: impl AsyncFnOnce(&Scenario, &mut Vec<f64>, &mut Map<String, Value>) -> Result<(), String>,
    ) -> Result<Value, String> {
        tokio::time::sleep(Duration::from_millis(IDLE_MS)).await;
        let (mut ops, mut extra) = (vec![], Map::new());
        self.probe.begin();
        let watch = Instant::now();
        self.log.mark(&format!("perf phase {name}"));
        self.host.log.line(&format!("perf scenario: phase {name}"));
        js(&self.host, "window.__perf.longTasks.length = 0").await?;
        let outcome = body(self, &mut ops, &mut extra).await;
        let stall = self.probe.take();
        self.log.mark("perf idle");
        outcome?;
        let long_tasks: String = serde_json::from_str(&js(&self.host, PAGE_LONG_TASKS).await?).unwrap_or_else(|_| "{}".into());
        let long_tasks = serde_json::from_str(&long_tasks).unwrap_or_else(|_| json!({}));
        Ok(phase_json(&ops, watch.elapsed().as_secs_f64() * 1000.0, &stall, extra, long_tasks))
    }
}

/// C# `ClickTab`.
fn click_tab(id: &str) -> String {
    format!(
        "const id = {}; const tab = document.querySelector('[data-tab-id=\"' + id + '\"] .tab-handle'); if (!tab) throw new Error('no tab ' + id); tab.click(); \
await __perf.until(() => {{ const s = document.querySelector('.pane-slot[data-pane-id=\"' + id + '\"]'); return !!s && !s.classList.contains('hidden') && !!s.querySelector('.pane') && !!s.querySelector('[data-testid=conversation], [data-testid=mighty]'); }}, t0);",
        literal(id)
    )
}

/// C# `TypeDraft`.
fn type_draft(text: &str) -> String {
    format!(
        "const text = {}; const e = __perf.slot()?.querySelector('[data-testid=composer-input]') ?? document.querySelector('[data-testid=composer-input]'); if (!e) throw new Error('no composer'); \
Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype, 'value').set.call(e, text); e.dispatchEvent(new Event('input', {{ bubbles: true }})); await __perf.until(() => e.value === text, t0);",
        literal(text)
    )
}

/// The add menu's Claude entry, until the new pane shows (C#'s `Add`).
const ADD_PANE: &str = "const known = new Set([...document.querySelectorAll('[data-testid=tab]')].map(e => e.dataset.tabId)); const entry = () => document.querySelector('[data-testid=\"add-entry-agent:claude\"]'); \
const plus = document.querySelector('[data-testid=sidebar-add]'); if (!plus) throw new Error('no add button'); plus.click(); await __perf.until(() => !!entry(), t0); entry().click(); \
await __perf.until(() => { const s = __perf.slot(); return !!s && !known.has(s.dataset.paneId) && !!s.querySelector('.pane'); }, t0);";

/// A tab's close, until the tab is gone (C#'s `Close`).
fn close_tab(id: &str) -> String {
    format!(
        "const id = {}; const b = document.querySelector('[data-tab-id=\"' + id + '\"] .tab-close'); if (!b) throw new Error('no close for ' + id); b.click(); \
await __perf.until(() => !document.querySelector('[data-tab-id=\"' + id + '\"]'), t0);",
        literal(id)
    )
}

/// Natural submit B1 (C#'s `SendAndSee`): Enter in the composer until the page draws the sent request, before the host has answered.
const SEND_AND_SEE: &str = "const e = __perf.slot()?.querySelector('[data-testid=composer-input]'); if (!e) throw new Error('no composer'); \
const drawn = () => [...(__perf.slot() ?? document).querySelectorAll('[data-pending-request]')].map(x => x.dataset.pendingRequest); const before = new Set(drawn()); \
e.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', code: 'Enter', keyCode: 13, bubbles: true, cancelable: true })); \
await __perf.until(() => drawn().some(id => !before.has(id)), t0);";

/// C# `RunPerfScenario`.
async fn run(host: Arc<Host>) {
    let directory = host.data_dir.clone();
    let result_path = directory.join("perf-result.json");
    let log = PerfLog(directory.join("perf.log"));
    let _ = std::fs::write(&log.0, "");
    log.mark("app started");
    trace(&directory, "perf scenario: the page is up");
    let errors: Arc<Mutex<Vec<String>>> = Arc::default();
    let mut result = result_skeleton(&local_o());
    let scenario = Scenario { host: host.clone(), log, probe: Probe::start(&host), errors: errors.clone() };
    let outcome = scenario_steps(&scenario, &mut result, &result_path).await;
    drop(scenario);
    if let Err(error) = &outcome {
        errors.lock().unwrap_or_else(|p| p.into_inner()).push(error.clone());
        result.insert("errors".into(), json!(errors.lock().unwrap_or_else(|p| p.into_inner()).clone()));
        write(&result_path, &result);
        // C# `WriteStartupFailure`: the run's own record of what failed.
        let failure = json!({"passed": false, "phase": "perf", "error": error, "aiRequestSent": false});
        let out = host.smoke.as_ref().map_or_else(|| directory.join("smoke-result.json"), |s| s.out().to_path_buf());
        let _ = std::fs::write(out, serde_json::to_vec_pretty(&failure).unwrap_or_default());
    }
    trace(&directory, if outcome.is_ok() { "perf scenario: finished" } else { "perf scenario: failed" });
    host.log.line(&format!("perf scenario: {}", if outcome.is_ok() { "completed" } else { "FAILED" }));
    if let Some(smoke) = &host.smoke {
        smoke.end(&host, outcome.is_ok());
    }
}

/// `DateTimeOffset.Now.ToString("O")`: the local time with its offset.
fn local_o() -> String {
    let now = chrono::Local::now();
    // chrono has no seven-digit fraction: .NET's ticks are written by hand.
    format!("{}.{:07}{}", now.format("%Y-%m-%dT%H:%M:%S"), chrono::Timelike::nanosecond(&now) % 1_000_000_000 / 100, now.format("%:z"))
}

fn write(path: &Path, result: &Map<String, Value>) {
    let _ = std::fs::write(path, serde_json::to_vec_pretty(&Value::Object(result.clone())).unwrap_or_default());
}

fn set_phase(result: &mut Map<String, Value>, name: &str, phase: Value) {
    if let Some(phases) = result.get_mut("phases").and_then(Value::as_object_mut) {
        phases.insert(name.into(), phase);
    }
}

/// The scenario itself (C# `RunPerfScenario`'s body), writing the result after each phase.
async fn scenario_steps(scenario: &Scenario, result: &mut Map<String, Value>, result_path: &Path) -> Result<(), String> {
    let host = &scenario.host;
    let errors = scenario.errors.clone();
    let write_now = |result: &mut Map<String, Value>| {
        result.insert("errors".into(), json!(errors.lock().unwrap_or_else(|p| p.into_inner()).clone()));
        write(result_path, result);
    };
    // The smoke's fixture providers: no CLI is read for a model list or a sign-in.
    let engine = host.engine()?;
    engine.use_fixture_runtime();
    host.panes_changed();
    let started = Instant::now();
    let (workspace, facts) = seed(host).await?;
    scenario.log.step("perf seed", started);
    host.log.line("perf scenario: seeded");
    for (key, value) in facts {
        result.insert(key, value);
    }
    change(host, |view| crate::frame::apply_layout_preset(view, "tabs"))?;
    let state = host.current();
    let active = state.active_session_id.clone().ok_or("the scenario has no active pane")?;
    let tabs = tabs_of(&state, &workspace, &active);
    // The page is ready when every tab of the group is drawn and the active pane shows.
    let ready = format!(
        "(() => {}.every(id => !!document.querySelector('[data-tab-id=\"' + id + '\"]')) && !!document.querySelector('.pane-slot.active:not(.hidden) .pane'))()",
        serde_json::to_string(&tabs).unwrap_or_default()
    );
    let deadline = Instant::now();
    while js(host, &ready).await? != "true" {
        if deadline.elapsed() > Duration::from_secs(30) {
            return Err("The page did not draw the scenario's tabs.".into());
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    host.log.line("perf scenario: the page drew the scenario's tabs");
    js(host, PAGE_HELPER).await?;
    let agents = state.sessions().iter().filter(|s| s.workspace_id() == workspace && s.kind() == "claude").count();
    let facts = PageFacts {
        panes: agents,
        tabs: tabs.len(),
        entries: state.sessions().iter().map(|s| s.logs().len()).sum(),
        active_entries: state.session(&active).map_or(0, |s| s.logs().len()),
        graph_runs: state.sessions().iter().map(|s| s.graph_runs().len()).sum(),
        window: window_size(host),
    };
    add_page_facts(result, &facts);
    write_now(result);

    let phase = scenario
        .phase("switchTabsMighty", async |s: &Scenario, ops: &mut Vec<f64>, _: &mut Map<String, Value>| {
            let now = s.host.current().active_session_id.clone().unwrap_or_else(|| active.clone());
            let at = tabs.iter().position(|t| *t == now).map_or(-1, |at| at as i64);
            for round in 0..ROUNDS {
                let next = (at + 1 + round as i64).rem_euclid(tabs.len().max(1) as i64) as usize;
                s.page_op(ops, &click_tab(&tabs[next])).await?;
            }
            s.page_op(&mut vec![], &click_tab(&active)).await?;
            settled(&s.host).await;
            Ok(())
        })
        .await?;
    set_phase(result, "switchTabsMighty", phase);
    write_now(result);

    let phase = scenario
        .phase("addPane", async |s: &Scenario, ops: &mut Vec<f64>, extra: &mut Map<String, Value>| {
            let before: Vec<String> = s.host.current().sessions().iter().map(|p| p.id().to_string()).collect();
            for _ in 0..ADDED_PANES {
                s.page_op(ops, ADD_PANE).await?;
            }
            let added: Vec<String> =
                s.host.current().sessions().iter().map(|p| p.id().to_string()).filter(|id| !before.contains(id)).collect();
            let mut closes = vec![];
            for id in &added {
                s.page_op(&mut closes, &close_tab(id)).await?;
            }
            extra.extend(add_pane_extra(&closes, added.len()));
            s.page_op(&mut vec![], &click_tab(&active)).await?;
            settled(&s.host).await;
            Ok(())
        })
        .await?;
    set_phase(result, "addPane", phase);
    write_now(result);

    let name = "streamMighty";
    let phase = scenario
        .phase(name, async |s: &Scenario, ops: &mut Vec<f64>, extra: &mut Map<String, Value>| {
            stream_answer(s, &active, name, extra, ops).await
        })
        .await?;
    set_phase(result, name, phase);
    write_now(result);
    scenario.page_op(&mut vec![], &type_draft("")).await?;

    // Natural submit B1: Enter in the composer until the page draws the sent request (the conversation's bubble, the diagram's request
    // block), before the host has answered; the target is one frame, 50 ms or less. The start goes to this exe's canned CLI, which puts
    // the request's graph run and user entry on the pane as a run does and ends at once, so the page's block gives way to them before the
    // next send. Nothing reaches a real CLI.
    let name = "sendBlockMighty";
    let script = send_script(&host.data_dir, name)?;
    let executable = std::env::current_exe().map_err(|e| format!("the app's own binary is not known: {e}"))?;
    let canned = CliCommand::new(
        executable.to_string_lossy().into_owned(),
        vec![mighty_delegation::help_demo_cli::ARGUMENT.into(), script.to_string_lossy().into_owned()],
        Some(mighty_delegation::help_demo_cli::VERSION),
    );
    engine.catalog.set_override("claude", Some(canned));
    let phase = scenario
        .phase(name, async |s: &Scenario, ops: &mut Vec<f64>, extra: &mut Map<String, Value>| {
            for turn in 1..=SENDS {
                s.page_op(&mut vec![], &type_draft(&format!("perf send {name} {turn}"))).await?;
                s.page_op(ops, SEND_AND_SEE).await?;
                // The run ends before the next send, so the next Enter starts a request rather than queuing one.
                let id = active.clone();
                let host = s.host.clone();
                crate::help_capture::wait(30.0, move || host.current().session(&id).is_some_and(|p| p.status() != "running")).await;
                settled(&s.host).await;
            }
            extra.extend(send_extra());
            Ok(())
        })
        .await;
    engine.catalog.set_override("claude", None);
    set_phase(result, name, phase?);
    let log_path = host.data_dir.join("perf.log");
    finish_result(result, &log_path, &std::fs::read_to_string(&log_path).unwrap_or_default(), &local_o());
    write_now(result);
    Ok(())
}

/// The canned CLI's script for the send phase: each send's request answered at once (a system line, the answer, the result).
fn send_script(directory: &Path, name: &str) -> Result<PathBuf, String> {
    let runs: Vec<Value> = (1..=SENDS)
        .map(|turn| {
            let session = format!("5a0e0b0b-0000-4000-8000-{turn:012}");
            let answer = format!("Sent {turn}.");
            json!({"provider": "claude", "input": format!("perf send {name} {turn}"), "frames": [
                {"type": "system", "subtype": "init", "session_id": session, "model": "claude-sonnet-5-5", "permissionMode": "default"},
                {"type": "assistant", "uuid": format!("msg_perf_{turn}"), "session_id": session, "message": {"id": format!("msg_perf_{turn}"), "role": "assistant", "model": "claude-sonnet-5-5", "content": [{"type": "text", "text": answer}]}},
                {"type": "result", "subtype": "success", "is_error": false, "session_id": session, "result": answer}
            ]})
        })
        .collect();
    let path = directory.join("perf-send-script.json");
    std::fs::write(&path, json!({"runs": runs}).to_string()).map_err(|e| e.to_string())?;
    Ok(path)
}

/// The window's content as C#'s `windowSize` gives it: `<width>x<height> @ <scale>`, in points.
fn window_size(host: &Host) -> String {
    let Some(window) = host.app.get_window(WINDOW) else { return String::new() };
    let scale = window.scale_factor().unwrap_or(1.0);
    let size = window.inner_size().map(|s| s.to_logical::<f64>(scale)).unwrap_or(tauri::LogicalSize::new(0.0, 0.0));
    format!("{:.0}x{:.0} @ {scale:.2}", size.width, size.height)
}

/// The active pane's tab group's panes (C#'s `PaneLayout.Groups(EffectiveLayout(...)).First(...)`).
fn tabs_of(state: &mighty_core::AppSnapshot, workspace: &str, active: &str) -> Vec<String> {
    let mut defaults = HashMap::new();
    let Some(tree) = mighty_core::shell::effective_layout(state, workspace, &mut defaults) else { return vec![active.to_string()] };
    mighty_core::layout::groups(&tree)
        .into_iter()
        .map(|group| group.session_ids.clone().unwrap_or_default().into_iter().flatten().collect::<Vec<String>>())
        .find(|ids| ids.iter().any(|id| id == active))
        .unwrap_or_else(|| vec![active.to_string()])
}

fn change(host: &Host, act: impl FnOnce(&mut crate::host::View) -> Result<(), String>) -> Result<(), String> {
    let result = {
        let mut view = host.view();
        act(&mut view)
    };
    host.flush();
    result
}

/// C# `StreamPerfAnswer`: a streamed answer on `session` through the run service's own event path, a running request put on the pane
/// first so the diagram follows it; every 500 ms the composer is typed into, which the phase times as its operation.
async fn stream_answer(s: &Scenario, session: &str, name: &str, extra: &mut Map<String, Value>, ops: &mut Vec<f64>) -> Result<(), String> {
    let engine = s.host.engine()?;
    let receive = |event: RunEvent| {
        let started = Instant::now();
        engine.service.receive_for_perf(event);
        s.log.step("perf receive", started);
    };
    let run_id = format!("perf-{name}");
    let input = format!("Perf: {name}");
    let running = MightyGraphRun {
        id: Some(run_id.clone()),
        input: Some(input.clone()),
        status: Some("running".into()),
        provider: Some("claude".into()),
        ..empty_run()
    };
    update_pane(&s.host, session, |pane| {
        let mut runs = pane.graph_runs.clone().unwrap_or_default();
        runs.push(running.clone());
        RunSession { graph_runs: Some(runs), ..pane.clone() }
    })?;
    receive(RunEvent::entry(
        session,
        LogEntry::new(wire::new_id(), "user", input.clone(), mighty_core::time::now_o(), Some("claude".into())),
    ));
    receive(RunEvent::state(session, "running", None));
    let entry = wire::new_id();
    let total = STREAM_SECONDS * EVENTS_PER_SECOND;
    let gap = 1000.0 / EVENTS_PER_SECOND as f64;
    let clock = Instant::now();
    let elapsed = || clock.elapsed().as_secs_f64() * 1000.0;
    let mut text = String::new();
    let (mut sent, mut late_sends) = (0u64, 0u64);
    let stream = async {
        for index in 0..total {
            let wait = index as f64 * gap - elapsed();
            if wait > 1.0 {
                tokio::time::sleep(Duration::from_secs_f64(wait / 1000.0)).await;
            } else if wait < -gap {
                late_sends += 1;
            }
            text.push_str(&if index % 25 == 0 {
                format!("\n### Section {}\n", index / 25 + 1)
            } else if index % 7 == 0 {
                format!("- item {index} with `code` and **bold** words\n")
            } else {
                format!("Line {index}: the answer goes on with a sentence long enough to wrap in a narrow pane.\n")
            });
            receive(RunEvent::entry(
                session,
                LogEntry::new(entry.clone(), "assistant", text.clone(), mighty_core::time::now_o(), Some("claude".into())),
            ));
            sent += 1;
            // The events reach the page as a run's do (the host's batched refresh).
            tokio::task::yield_now().await;
        }
    };
    let typing = async {
        let mut turn = 1u32;
        while elapsed() < STREAM_SECONDS as f64 * 1000.0 - TYPING_EVERY_MS {
            let wait = f64::from(turn) * TYPING_EVERY_MS - elapsed();
            if wait > 1.0 {
                tokio::time::sleep(Duration::from_secs_f64(wait / 1000.0)).await;
            }
            s.page_op(ops, &type_draft(&format!("perf draft {turn}"))).await?;
            turn += 1;
        }
        Ok::<(), String>(())
    };
    let ((), typed) = tokio::join!(stream, typing);
    typed?;
    receive(RunEvent::state(session, "completed", None));
    extra.extend(stream_extra(sent, late_sends, elapsed()));
    settled(&s.host).await;
    // The request is put down as finished, so the next phase's diagram has nothing running.
    update_pane(&s.host, session, |pane| {
        let runs = pane
            .graph_runs()
            .iter()
            .map(|r| {
                if r.id.as_deref() == Some(run_id.as_str()) {
                    MightyGraphRun {
                        id: r.id.clone(),
                        input: r.input.clone(),
                        status: Some("completed".into()),
                        provider: Some("claude".into()),
                        root_entries: r.root_entries.clone(),
                        final_output: Some(text.clone()),
                        ..empty_run()
                    }
                } else {
                    r.clone()
                }
            })
            .collect();
        RunSession { graph_runs: Some(runs), ..pane.clone() }
    })
}

/// A graph run with C#'s empty lists (`new MightyGraphRun { … }`: its agents and entries are never null).
fn empty_run() -> MightyGraphRun {
    MightyGraphRun { root_entries: Some(vec![]), agents: Some(vec![]), result_entries: Some(vec![]), ..MightyGraphRun::default() }
}

fn update_pane(host: &Host, id: &str, edit: impl Fn(&RunSession) -> RunSession) -> Result<(), String> {
    change(host, |view| {
        view.update(|state| {
            for pane in state.sessions_mut().iter_mut().filter(|p| p.id() == id) {
                *pane = Arc::new(edit(pane));
            }
        })
    })
}

// ── seeding ──

/// The scenario's workspace and panes (C# `SeedPerfScenario`): the copied state's local agent panes, then made-up ones until the busiest
/// workspace has four Claude panes; the busiest conversation made active.
async fn seed(host: &Arc<Host>) -> Result<(String, Map<String, Value>), String> {
    let directory = host.data_dir.clone();
    let folders = directory.join("perf-workspaces");
    let mut from: Option<(String, usize)> = None;
    let mut imported: Vec<RunSession> = vec![];
    if let Some(source) = std::env::var("MIGHTY_PERF_STATE").ok().filter(|v| !v.is_empty()) {
        // Read from a copy, so the scenario never opens the given folder's state for writing; every workspace becomes a folder in the
        // profile (no project of the user's is read) and only agent panes come over (a terminal pane would start a process).
        let copy = directory.join("perf-import");
        std::fs::create_dir_all(&copy).map_err(|e| e.to_string())?;
        std::fs::copy(Path::new(&source).join(mighty_core::store::STATE_FILE), copy.join(mighty_core::store::STATE_FILE))
            .map_err(|e| e.to_string())?;
        let mut store = mighty_core::store::Store::new(copy, None, false);
        let snapshot = store.load(&host.locale).map_err(|e| e.to_string())?;
        let mut ids: HashMap<String, String> = HashMap::new();
        for workspace in snapshot.workspaces().iter().filter(|w| w.remote.is_none()) {
            ids.insert(workspace.id().to_string(), folder(host, &folders, workspace.name())?);
        }
        imported = snapshot
            .sessions()
            .iter()
            .filter(|s| s.kind() == "claude" && ids.contains_key(s.workspace_id()))
            .map(|s| {
                // A request the copy caught running would keep its block animating through every phase: it is put down as stopped.
                let runs = s.graph_runs.clone().map(|runs| {
                    runs.into_iter()
                        .map(|mut r| {
                            if !mighty_core::graph::terminal(r.status.as_deref().unwrap_or("")) {
                                r.status = Some("stopped".into());
                            }
                            r
                        })
                        .collect()
                });
                let status = if matches!(s.status(), "idle" | "completed" | "error" | "stopped") {
                    s.status().to_string()
                } else {
                    "completed".into()
                };
                RunSession {
                    workspace_id: Some(ids[s.workspace_id()].clone()),
                    status: Some(status),
                    draft: Some(String::new()),
                    current_activity: None,
                    parent_session_id: None,
                    working_folder: None,
                    graph_runs: runs,
                    ..(**s).clone()
                }
            })
            .collect();
        let added = imported.clone();
        change(host, |view| view.update(|state| state.sessions_mut().extend(added.into_iter().map(Arc::new))))?;
        from = Some((source, ids.len()));
    }
    let busiest = {
        let mut counts: Vec<(String, usize)> = vec![];
        for session in &imported {
            match counts.iter_mut().find(|(id, _)| id == session.workspace_id()) {
                Some((_, count)) => *count += 1,
                None => counts.push((session.workspace_id().to_string(), 1)),
            }
        }
        counts.into_iter().fold(None::<(String, usize)>, |best, next| match best {
            Some(best) if best.1 >= next.1 => Some(best),
            _ => Some(next),
        })
    };
    let workspace = match busiest {
        Some((id, _)) => id,
        None => folder(host, &folders, "Perf project")?,
    };
    let claude = host
        .current()
        .sessions()
        .iter()
        .filter(|s| s.workspace_id() == workspace && s.kind() == "claude" && s.provider() == "claude")
        .count();
    // With the owner's state, only the owner's panes are measured; made-up panes fill in only a run without one.
    let made: Vec<RunSession> =
        if imported.is_empty() { (0..4usize.saturating_sub(claude)).map(|index| perf_pane(&workspace, index)).collect() } else { vec![] };
    let count = made.len();
    if !made.is_empty() {
        change(host, |view| view.update(|state| state.sessions_mut().extend(made.into_iter().map(Arc::new))))?;
    }
    let facts = seed_facts(from.as_ref().map(|(source, workspaces)| (source.as_str(), *workspaces, imported.len())), count);
    // The busiest conversation is the one every phase works on.
    let active = {
        let state = host.current();
        let mut panes: Vec<&Arc<RunSession>> =
            state.sessions().iter().filter(|s| s.workspace_id() == workspace && s.kind() == "claude").collect();
        panes.sort_by_key(|s| std::cmp::Reverse(s.logs().len()));
        panes.first().map(|s| s.id().to_string()).ok_or("the scenario has no Claude pane")?
    };
    let chosen = workspace.clone();
    change(host, |view| {
        view.update(|state| {
            state.active_workspace_id = Some(chosen.clone());
            state.active_session_id = Some(active.clone());
        })
    })?;
    Ok((workspace, facts))
}

/// `Path.GetInvalidFileNameChars()` on Windows.
fn invalid_file_name_char(c: char) -> bool {
    (c as u32) < 32 || matches!(c, '"' | '<' | '>' | '|' | ':' | '*' | '?' | '\\' | '/')
}

/// A folder of the scenario's own, numbered in the order made, listed as a workspace (C#'s `Folder(name)`): its id.
fn folder(host: &Host, folders: &Path, name: &str) -> Result<String, String> {
    let safe: String = name.chars().map(|c| if invalid_file_name_char(c) { '_' } else { c }).collect::<String>().trim().to_string();
    let index = std::fs::read_dir(folders).map(|entries| entries.flatten().filter(|e| e.path().is_dir()).count()).unwrap_or(0);
    let path = folders.join(format!("{index:02} {}", if safe.is_empty() { "workspace" } else { &safe }));
    std::fs::create_dir_all(&path).map_err(|e| e.to_string())?;
    let workspace = host.approve_local(&path).map_err(|e| e.to_string())?;
    let id = workspace.id().to_string();
    change(host, |view| {
        view.update(|state| {
            if !state.workspaces().iter().any(|w| w.id() == id) {
                state.workspaces_mut().push(workspace.clone());
            }
        })
    })?;
    Ok(id)
}

/// A made-up Claude pane (C# `PerfPane`): 300 entries (requests, Markdown answers with headings, lists and code, tool calls) and 15
/// finished requests for the diagram.
pub fn perf_pane(workspace: &str, pane: usize) -> RunSession {
    let now = mighty_core::time::now_o;
    let (mut logs, mut runs) = (vec![], vec![]);
    for turn in 0..75 {
        let request = format!("Pane {}, request {}: look at the module and fix what is slow.", pane + 1, turn + 1);
        let answer = format!(
            "## Step {}\n\nI read the code and changed **three** things:\n\n- the cache now keeps `{turn}` entries\n- the loop no longer copies the list\n- a test covers it\n\n```csharp\nfor (var i = 0; i < {turn}; i++)\n    total += values[i];\n```\n\n> The rest stays as it was.",
            turn + 1
        );
        let tool = AgentActivity {
            id: format!("perf-{pane}-{turn}"),
            provider: "claude".into(),
            kind: "command".into(),
            state: "completed".into(),
            summary: "dotnet test".into(),
            tool_name: None,
            output: Some("42 tests passed".into()),
            duration_ms: Some(f64::from(1200 + turn)),
        };
        let claude = Some("claude".to_string());
        let mut system = LogEntry::new(wire::new_id(), "system", tool.summary.clone(), now(), claude.clone());
        system.activity = Some(tool);
        let entries = [
            LogEntry::new(wire::new_id(), "user", request.clone(), now(), claude.clone()),
            system,
            LogEntry::new(wire::new_id(), "assistant", answer.clone(), now(), claude.clone()),
            LogEntry::new(wire::new_id(), "assistant", format!("Done with step {}.", turn + 1), now(), claude.clone()),
        ];
        logs.extend(entries.iter().cloned());
        if turn >= 60 {
            runs.push(MightyGraphRun {
                id: Some(format!("perf-run-{pane}-{turn}")),
                input: Some(request),
                status: Some("completed".into()),
                provider: claude,
                root_entries: Some(entries[1..].to_vec()),
                final_output: Some(answer),
                ..empty_run()
            });
        }
    }
    RunSession {
        id: Some(wire::new_id()),
        workspace_id: Some(workspace.to_string()),
        kind: Some("claude".into()),
        provider: Some("claude".into()),
        title: Some(format!("Perf {}", pane + 1)),
        status: Some("completed".into()),
        logs: Some(logs),
        graph_runs: Some(runs),
        ..RunSession::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// C# `PerfStalls`: lateness beyond 33 ms summed, the ticks over 100 and 250 counted, the worst kept; rounded to a tenth.
    #[test]
    fn stalls_count_as_csharps_probe_does() {
        let mut stalls = Stalls::default();
        for late in [15.0, 40.0, 120.5, 300.04] {
            stalls.add(late);
        }
        assert_eq!(stalls.json(), json!({"ticks": 4, "lateBeyond33SumMs": 361.5, "maxLateMs": 300.0, "over100": 2, "over250": 1}));
    }

    /// C#'s `Rank`: the ceiling of the share of the count, less one, clamped.
    #[test]
    fn ranks_are_csharps() {
        let sorted: Vec<f64> = (1..=20).map(f64::from).collect();
        assert_eq!(rank(&sorted, 0.5), 10.0);
        assert_eq!(rank(&sorted, 0.95), 19.0);
        assert_eq!(rank(&[7.0], 0.95), 7.0);
        assert_eq!(rank(&[], 0.5), 0.0);
    }

    /// C# `PerfLogSteps`: the steps between `perf phase` marks added up under that phase and under `all`, the marks themselves not counted,
    /// each phase's steps largest total first.
    #[test]
    fn perf_log_steps_add_up_per_phase() {
        let log = "10:00:00.000     mark    app started\n\
10:00:00.100     20.0 ms perf seed\n\
10:00:01.000     mark    perf phase streamMighty\n\
10:00:01.100     16.5 ms perf receive\n\
10:00:01.200     30,5 ms perf receive\n\
10:00:01.300    100.0 ms other\n\
10:00:02.000     mark    perf idle\n\
10:00:02.100     17.0 ms perf receive\n";
        let steps = log_steps(log);
        assert_eq!(steps.keys().collect::<Vec<_>>(), ["setup", "all", "streamMighty", "idle"]);
        assert_eq!(steps["setup"], json!({"perf seed": {"count": 1, "totalMs": 20.0, "maxMs": 20.0}}));
        assert_eq!(steps["streamMighty"].as_object().unwrap().keys().collect::<Vec<_>>(), ["other", "perf receive"]);
        assert_eq!(steps["streamMighty"]["perf receive"], json!({"count": 2, "totalMs": 47.0, "maxMs": 30.5}));
        assert_eq!(steps["all"]["perf receive"], json!({"count": 3, "totalMs": 64.0, "maxMs": 30.5}));
        assert!(log_steps("").is_empty());
    }

    /// The perf log's lines read back as written: a mark and a step in C#'s columns.
    #[test]
    fn the_perf_log_writes_what_log_steps_reads() {
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let path = std::env::temp_dir().join(format!("mighty-perf-log-{}-{nanos}.log", std::process::id()));
        let log = PerfLog(path.clone());
        log.mark("perf phase addPane");
        log.step("slow step", Instant::now() - Duration::from_millis(40));
        log.step("quick step", Instant::now());
        log.mark("perf idle");
        let steps = log_steps(&std::fs::read_to_string(&path).unwrap());
        assert_eq!(steps["addPane"]["slow step"]["count"], 1);
        assert!(steps["addPane"].get("quick step").is_none(), "a step under 16 ms is not written");
        let _ = std::fs::remove_file(path);
    }

    /// C# `PerfPane`: 300 entries (each turn a request, a tool call, an answer and a closing line) and the last 15 turns as finished
    /// requests whose entries are the turn's answer side.
    #[test]
    fn a_made_up_pane_is_csharps() {
        let pane = perf_pane("w", 2);
        assert_eq!(pane.title.as_deref(), Some("Perf 3"));
        assert_eq!(pane.logs().len(), 300);
        assert_eq!(pane.graph_runs().len(), 15);
        assert_eq!(pane.logs()[1].activity.as_ref().map(|a| a.summary.as_str()), Some("dotnet test"));
        let first = &pane.graph_runs()[0];
        assert_eq!(first.id.as_deref(), Some("perf-run-2-60"));
        assert_eq!(first.root_entries.as_ref().map(Vec::len), Some(3));
        assert!(pane.graph_runs().iter().all(|r| r.status.as_deref() == Some("completed")));
    }

    /// A JSON value's object keys, and each object member's own, as paths in document order (arrays' items by their first item).
    fn key_paths(value: &Value, prefix: &str, out: &mut Vec<String>) {
        match value {
            Value::Object(map) => {
                for (key, member) in map {
                    let path = format!("{prefix}/{key}");
                    out.push(path.clone());
                    // perfLogSteps' buckets and steps are the log's own names, not keys of the format.
                    if key != "perfLogSteps" {
                        key_paths(member, &path, out);
                    }
                }
            }
            Value::Array(items) => {
                if let Some(first) = items.first() {
                    key_paths(first, &format!("{prefix}[]"), out);
                }
            }
            _ => {}
        }
    }

    /// The result's keys are C#'s (review M2): a result built by the scenario's own builders, the phases in its order with each
    /// phase's own additions, has exactly the keys, in the order, of a C# 0.4.59 run's `perf-result.json`
    /// (`native/contracts/fixtures/perf-result-csharp-0.4.59.json`, a made-up run without `MIGHTY_PERF_STATE`, the one shape C# writes
    /// here), down to every phase's `stall`, `pageLongTasks` and `gc`.
    #[test]
    fn the_result_has_csharps_keys_in_csharps_order() {
        let fixture: Value =
            serde_json::from_str(include_str!("../../../contracts/fixtures/perf-result-csharp-0.4.59.json")).expect("the C# result reads");
        let mut result = result_skeleton(&local_o());
        result.extend(seed_facts(None, 4));
        let page = PageFacts { panes: 4, tabs: 4, entries: 1200, active_entries: 300, graph_runs: 60, window: "1440x920 @ 1.00".into() };
        add_page_facts(&mut result, &page);
        let mut stall = Stalls::default();
        stall.add(20.0);
        let long_tasks = json!({"observed": true, "count": 0, "sumMs": 0.0, "maxMs": 0.0, "over100": 0, "over250": 0});
        let bodies = [
            ("switchTabsMighty", Map::new()),
            ("addPane", add_pane_extra(&[12.0], 1)),
            ("streamMighty", stream_extra(200, 0, 8000.0)),
            ("sendBlockMighty", send_extra()),
        ];
        for (name, body) in bodies {
            let phase = phase_json(&[10.0, 20.0], 1000.0, &stall, body, long_tasks.clone());
            result["phases"].as_object_mut().unwrap().insert(name.into(), phase);
        }
        finish_result(&mut result, Path::new("perf.log"), "10:00:00.000     20.0 ms perf seed\n", &local_o());
        let (mut ours, mut theirs) = (vec![], vec![]);
        key_paths(&Value::Object(result), "", &mut ours);
        key_paths(&fixture, "", &mut theirs);
        assert_eq!(ours, theirs);
        // With MIGHTY_PERF_STATE, C#'s three facts come before synthesizedPanes.
        let imported: Vec<String> = seed_facts(Some(("C:\\state", 2, 9)), 0).keys().cloned().collect();
        assert_eq!(imported, ["importedFrom", "importedWorkspaces", "importedPanes", "synthesizedPanes"]);
    }

    /// The send script answers each send.
    #[test]
    fn the_send_script_answers_each_send() {
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let dir = std::env::temp_dir().join(format!("mighty-perf-send-{}-{nanos}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let script: Value = serde_json::from_slice(&std::fs::read(send_script(&dir, "sendBlockMighty").unwrap()).unwrap()).unwrap();
        let runs = script["runs"].as_array().unwrap();
        assert_eq!(runs.len(), SENDS);
        assert_eq!(runs[9]["input"], "perf send sendBlockMighty 10");
        assert_eq!(runs[0]["frames"].as_array().unwrap().last().unwrap()["type"], "result");
        let _ = std::fs::remove_dir_all(dir);
    }

    /// `DateTimeOffset.Now.ToString("O")`'s shape: seven fraction digits and the offset.
    #[test]
    fn local_times_are_written_as_dotnets_o() {
        let text = local_o();
        assert_eq!(text.len(), "2026-11-15T19:56:51.5265328+09:00".len(), "{text}");
        assert_eq!(&text[19..20], ".");
        assert!(text[20..27].chars().all(|c| c.is_ascii_digit()));
    }

    #[test]
    fn invalid_file_name_characters_are_windows() {
        let safe: String = "a<b>:c/d\\e|f?g*h\"i\u{1}".chars().map(|c| if invalid_file_name_char(c) { '_' } else { c }).collect();
        assert_eq!(safe, "a_b__c_d_e_f_g_h_i_");
    }
}
