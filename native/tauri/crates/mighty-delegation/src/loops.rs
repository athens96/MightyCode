//! MC-118: the loop skeleton of 0.7.0 (docs/loops-contract.md). A loop drives one agent pane toward a goal: it sends the goal, and
//! after each turn the host runs the loop's checks; while one fails it sends a repair request to the same pane and checks again, until
//! every criterion passes with a clean guard or a stop condition is met.
//!
//! This module is the part with no app in it: the record the page draws ([`LoopRecord`] and its parts), the criteria's checks and
//! their hash, the guard that catches a loop gaming its checks (protected test files, tests removed, skips added; [`GuardBaseline`]),
//! the prompts, and the driver ([`drive`]) over a [`LoopHost`] the app implements (src-tauri `loops.rs`).

use std::collections::BTreeMap;
use std::future::Future;

use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

use crate::verify::{self, CommandResult, RoundResult};

/// The loops kept in the list (newest first).
pub const MAXIMUM_LOOPS_KEPT: usize = 50;
/// The most criteria a loop may have.
pub const MAXIMUM_CRITERIA: usize = 20;
/// The most characters of a criterion's text and of a command.
pub const MAXIMUM_CRITERION_CHARS: usize = 400;
/// The most characters of a title.
pub const MAXIMUM_TITLE_CHARS: usize = 80;
/// The most characters of a goal.
pub const MAXIMUM_GOAL_CHARS: usize = 20_000;
/// The protected files read for the guard at most; a repository with more protects the first ones (sorted).
pub const MAXIMUM_PROTECTED_FILES: usize = 4_000;
/// A protected file larger than this is hashed but not counted for tests.
pub const MAXIMUM_COUNTED_BYTES: usize = 1_000_000;

// ── the record ───────────────────────────────────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum LoopState {
    Draft,
    Running,
    Paused,
    Passed,
    Failed,
    Stopped,
}

impl LoopState {
    pub fn finished(self) -> bool {
        matches!(self, LoopState::Passed | LoopState::Failed | LoopState::Stopped)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum LoopPhase {
    Build,
    Verify,
    Repair,
    Done,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum LoopStop {
    Passed,
    MaxIterations,
    MaxMinutes,
    MaxCost,
    Budget,
    SameFailure,
    Person,
    RepairEnded,
    PaneClosed,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Limits {
    pub max_iterations: u32,
    pub max_minutes: u32,
    pub max_cost_usd: f64,
}

impl Default for Limits {
    fn default() -> Self {
        Limits { max_iterations: 10, max_minutes: 120, max_cost_usd: 5.0 }
    }
}

impl Limits {
    /// The limits kept within what a loop may ask: 1–50 iterations, 1 minute to a day, 0.1 to 500 USD.
    pub fn clamped(self) -> Limits {
        let cost = if self.max_cost_usd.is_finite() { self.max_cost_usd } else { Limits::default().max_cost_usd };
        Limits {
            max_iterations: self.max_iterations.clamp(1, 50),
            max_minutes: self.max_minutes.clamp(1, 24 * 60),
            max_cost_usd: cost.clamp(0.1, 500.0),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CheckKind {
    /// A shell command in the pane's run folder; passes on exit 0.
    Command,
    /// One of the approved `.mighty.yaml` `verify` commands (`name`), or its browser check (`name: "browser"`).
    VerifyConfig,
    /// The agent's own checklist item: informational, never makes a loop pass on its own.
    Checklist,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Check {
    pub kind: CheckKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
}

/// The browser check's name among the `verifyConfig` criteria.
pub const BROWSER_CHECK: &str = "browser";

/// The evidence ladder's rung a criterion must reach (the wiki's L1–L4; L5, an independent verifier, comes in 0.7.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Level {
    L1,
    L2,
    L3,
    L4,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CriterionStatus {
    Pending,
    Passed,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Evidence {
    pub level: Level,
    pub summary: String,
    pub at: String,
    pub duration_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Criterion {
    #[serde(default)]
    pub id: String,
    pub text: String,
    pub check: Check,
    pub level: Level,
    #[serde(default = "pending")]
    pub status: CriterionStatus,
    #[serde(default)]
    pub evidence: Option<Evidence>,
}

fn pending() -> CriterionStatus {
    CriterionStatus::Pending
}

impl Criterion {
    /// Whether this criterion can make a loop pass (a command the app runs).
    pub fn decides(&self) -> bool {
        matches!(self.check.kind, CheckKind::Command | CheckKind::VerifyConfig)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum IterationKind {
    Build,
    Repair,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum IterationOutcome {
    Running,
    Passed,
    Failed,
    Stopped,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Iteration {
    pub n: u32,
    pub kind: IterationKind,
    #[serde(default)]
    pub run_id: Option<String>,
    pub started_at: String,
    #[serde(default)]
    pub finished_at: Option<String>,
    pub outcome: IterationOutcome,
    #[serde(default)]
    pub failed: Vec<String>,
    #[serde(default)]
    pub changed_files: u32,
    #[serde(default)]
    pub added: u32,
    #[serde(default)]
    pub removed: u32,
    #[serde(default)]
    pub cost_usd: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum GuardFlag {
    TestsRemoved,
    SkipAdded,
    ProtectedChanged,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Guard {
    pub protected_files: usize,
    #[serde(default)]
    pub touched: Vec<String>,
    pub tests_at_start: usize,
    pub tests_now: usize,
    #[serde(default)]
    pub skips_added: usize,
    #[serde(default)]
    pub flags: Vec<GuardFlag>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LoopRecord {
    pub id: String,
    pub pane_id: String,
    pub workspace_id: String,
    pub title: String,
    pub goal: String,
    pub state: LoopState,
    pub phase: LoopPhase,
    pub criteria: Vec<Criterion>,
    #[serde(default)]
    pub criteria_hash: String,
    pub limits: Limits,
    #[serde(default)]
    pub iterations: Vec<Iteration>,
    #[serde(default)]
    pub guard: Guard,
    #[serde(default)]
    pub cost_usd: f64,
    #[serde(default)]
    pub started_at: Option<String>,
    #[serde(default)]
    pub finished_at: Option<String>,
    #[serde(default)]
    pub stop_reason: Option<LoopStop>,
    #[serde(default)]
    pub base_commit: Option<String>,
}

impl LoopRecord {
    /// A draft for `pane_id` with `criteria` proposed and the default limits.
    pub fn draft(id: String, pane_id: &str, workspace_id: &str, goal: &str, criteria: Vec<Criterion>) -> LoopRecord {
        LoopRecord {
            id,
            pane_id: pane_id.to_string(),
            workspace_id: workspace_id.to_string(),
            title: title_from_goal(goal),
            goal: goal.to_string(),
            state: LoopState::Draft,
            phase: LoopPhase::Build,
            criteria,
            criteria_hash: String::new(),
            limits: Limits::default(),
            iterations: Vec::new(),
            guard: Guard::default(),
            cost_usd: 0.0,
            started_at: None,
            finished_at: None,
            stop_reason: None,
            base_commit: None,
        }
    }

    /// The record as the app came back from a restart: a loop that was running is paused (the person resumes it).
    pub fn after_restart(mut self) -> LoopRecord {
        if self.state == LoopState::Running {
            self.state = LoopState::Paused;
            if let Some(last) = self.iterations.last_mut()
                && last.outcome == IterationOutcome::Running
            {
                last.outcome = IterationOutcome::Stopped;
            }
        }
        self
    }

    /// Whether the criteria still are the ones approved at start.
    pub fn criteria_intact(&self) -> bool {
        self.criteria_hash == criteria_hash(&self.criteria)
    }

    /// The first criterion that failed last, for the pane's band.
    pub fn first_failing(&self) -> Option<&Criterion> {
        self.criteria.iter().find(|c| c.status == CriterionStatus::Failed)
    }
}

/// The title of a goal: its first line with words, at most [`MAXIMUM_TITLE_CHARS`].
pub fn title_from_goal(goal: &str) -> String {
    let line = goal.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or_default();
    verify::cap_chars(line, MAXIMUM_TITLE_CHARS)
}

/// The hash of the approved criteria: what each checks and how far (16 hex characters of a SHA-256); the statuses and evidence are not
/// in it.
pub fn criteria_hash(criteria: &[Criterion]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"mighty-loop-criteria-v1\0");
    for c in criteria {
        let kind = match c.check.kind {
            CheckKind::Command => "command",
            CheckKind::VerifyConfig => "verifyConfig",
            CheckKind::Checklist => "checklist",
        };
        for part in [
            c.id.as_str(),
            c.text.as_str(),
            kind,
            c.check.name.as_deref().unwrap_or(""),
            c.check.command.as_deref().unwrap_or(""),
            c.check.cwd.as_deref().unwrap_or(""),
            &format!("{:?}", c.level),
        ] {
            hasher.update(part.as_bytes());
            hasher.update(b"\0");
        }
        hasher.update(b"\x1e");
    }
    hasher.finalize().iter().take(8).map(|b| format!("{b:02x}")).collect()
}

/// Why criteria cannot start a loop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CriteriaError {
    /// None of them is a command or a `verify` command.
    NoCommand,
    /// More than [`MAXIMUM_CRITERIA`].
    TooMany,
    /// A `command` criterion without a command, or a `verifyConfig` one without a name.
    Incomplete,
}

/// The criteria as a loop keeps them: text and commands trimmed and capped, an id for each (`c1`, `c2`, … where one is missing or
/// repeated), every status pending, no evidence, a checklist item at L1. Err when they cannot start a loop.
pub fn normalize_criteria(criteria: Vec<Criterion>) -> Result<Vec<Criterion>, CriteriaError> {
    if criteria.len() > MAXIMUM_CRITERIA {
        return Err(CriteriaError::TooMany);
    }
    let mut out: Vec<Criterion> = Vec::with_capacity(criteria.len());
    for (index, mut c) in criteria.into_iter().enumerate() {
        c.text = verify::cap_chars(c.text.trim(), MAXIMUM_CRITERION_CHARS);
        c.check.name = c.check.name.map(|n| n.trim().to_string()).filter(|n| !n.is_empty());
        c.check.command = c.check.command.map(|n| verify::cap_chars(n.trim(), MAXIMUM_CRITERION_CHARS)).filter(|n| !n.is_empty());
        c.check.cwd = c.check.cwd.map(|n| n.trim().to_string()).filter(|n| !n.is_empty());
        match c.check.kind {
            CheckKind::Command if c.check.command.is_none() => return Err(CriteriaError::Incomplete),
            CheckKind::VerifyConfig if c.check.name.is_none() => return Err(CriteriaError::Incomplete),
            CheckKind::Checklist => c.level = Level::L1,
            _ => {}
        }
        if c.text.is_empty() {
            c.text = c.check.name.clone().or_else(|| c.check.command.clone()).unwrap_or_default();
        }
        let id = c.id.trim().to_string();
        if id.is_empty() || id.len() > 32 || out.iter().any(|o| o.id == id) {
            let mut n = index + 1;
            while out.iter().any(|o| o.id == format!("c{n}")) {
                n += 1;
            }
            c.id = format!("c{n}");
        } else {
            c.id = id;
        }
        c.status = CriterionStatus::Pending;
        c.evidence = None;
        out.push(c);
    }
    if !out.iter().any(Criterion::decides) {
        return Err(CriteriaError::NoCommand);
    }
    Ok(out)
}

/// A criterion for `name = command` (the CLI's `--check`): the text is the name.
pub fn command_criterion(name: &str, command: &str) -> Criterion {
    Criterion {
        id: String::new(),
        text: name.to_string(),
        check: Check { kind: CheckKind::Command, name: Some(name.to_string()), command: Some(command.to_string()), cwd: None },
        level: Level::L2,
        status: CriterionStatus::Pending,
        evidence: None,
    }
}

/// The level a `.mighty.yaml` command's name suggests: build, type or lint checks are L1, tests L2, anything that runs it for real L4.
pub fn level_for_name(name: &str) -> Level {
    let lower = name.to_lowercase();
    let has = |words: &[&str]| words.iter().any(|w| lower.contains(w));
    if has(&["e2e", "smoke", "run", "serve", "browser", "integration"]) {
        Level::L4
    } else if has(&["test", "spec", "check"]) && !has(&["typecheck", "type-check"]) {
        Level::L2
    } else {
        Level::L1
    }
}

// ── the guard ────────────────────────────────────────────────────────────────────────────────────────────────────────────

/// Whether `path` (relative, `/`-separated) is a test file: in a `test`, `tests`, `__tests__`, `spec` or `e2e` folder, or named like
/// one (`*.test.*`, `*.spec.*`, `*_test.*`, `test_*.py`, `*Test.java`, `*Tests.swift`, …).
pub fn is_test_path(path: &str) -> bool {
    let lower = path.replace('\\', "/").to_lowercase();
    let parts: Vec<&str> = lower.split('/').collect();
    let Some((file, folders)) = parts.split_last() else { return false };
    if folders.iter().any(|f| matches!(*f, "test" | "tests" | "__tests__" | "spec" | "specs" | "e2e" | "__snapshots__")) {
        return is_source_like(file);
    }
    let stem = file.rsplit_once('.').map_or(*file, |(s, _)| s);
    is_source_like(file)
        && (stem.ends_with(".test")
            || stem.ends_with(".spec")
            || stem.ends_with("_test")
            || stem.ends_with("_spec")
            || (stem.ends_with("tests") && original_case_suffix(path, "Tests"))
            || (stem.ends_with("test") && original_case_suffix(path, "Test"))
            || (file.starts_with("test_") && file.ends_with(".py")))
}

/// Whether the original path's stem ends with `suffix` in that case (`FooTest.java`, not `latest.ts`).
fn original_case_suffix(path: &str, suffix: &str) -> bool {
    let file = path.rsplit(['/', '\\']).next().unwrap_or(path);
    let stem = file.rsplit_once('.').map_or(file, |(s, _)| s);
    stem.ends_with(suffix)
}

fn is_source_like(file: &str) -> bool {
    let Some((_, ext)) = file.rsplit_once('.') else { return false };
    matches!(
        ext,
        "rs" | "ts"
            | "tsx"
            | "js"
            | "jsx"
            | "mjs"
            | "cjs"
            | "py"
            | "go"
            | "java"
            | "kt"
            | "swift"
            | "rb"
            | "cs"
            | "php"
            | "c"
            | "cc"
            | "cpp"
            | "snap"
    )
}

/// Whether `path` is a verification file the loop keeps locked: the repository config and the CI workflows.
pub fn is_verify_file(path: &str) -> bool {
    let p = path.replace('\\', "/");
    let file = p.rsplit('/').next().unwrap_or(&p);
    matches!(file, ".mighty.yaml" | ".mighty.yml" | ".mighty.local.yaml") || p.starts_with(".github/workflows/")
}

/// Whether the guard protects `path`.
pub fn is_protected(path: &str) -> bool {
    is_test_path(path) || is_verify_file(path)
}

/// The tests declared in `text`: lines that start a test in the common frameworks (`#[test]`, `it(`, `test(`, `def test_`,
/// `func Test`, `@Test`, `func test…` in XCTest, …).
pub fn count_tests(text: &str) -> usize {
    text.lines()
        .map(str::trim_start)
        .filter(|l| {
            l.starts_with("#[test]")
                || l.starts_with("#[tokio::test")
                || l.starts_with("#[rstest")
                || starts_call(l, &["it", "test", "it.each", "test.each", "it.only", "test.only", "it.concurrent", "test.concurrent"])
                || l.starts_with("def test_")
                || l.starts_with("async def test_")
                || l.starts_with("func Test")
                || l.starts_with("@Test")
                || l.starts_with("@ParameterizedTest")
                || (l.starts_with("func test") && l.contains("()"))
                || l.starts_with("[Fact")
                || l.starts_with("[Test]")
                || l.starts_with("[TestMethod")
        })
        .count()
}

/// The lines of `text` that skip a test: `#[ignore]`, `.skip(`, `xit(`, `it.todo(`, `@pytest.mark.skip`, `t.Skip(`, `@Disabled`, `XCTSkip`, ….
pub fn count_skips(text: &str) -> usize {
    const MARKERS: [&str; 11] = [
        "#[ignore",
        ".skip(",
        ".skipIf(",
        ".todo(",
        "@pytest.mark.skip",
        "@unittest.skip",
        "t.Skip(",
        "@Disabled",
        "@Ignore",
        "XCTSkip",
        "[Ignore",
    ];
    text.lines()
        .map(str::trim_start)
        .filter(|l| MARKERS.iter().any(|m| l.contains(m)) || starts_call(l, &["xit", "xtest", "xdescribe"]))
        .count()
}

/// Whether `line` starts with one of `names` followed by `(` (or a template literal for `.each`).
fn starts_call(line: &str, names: &[&str]) -> bool {
    names
        .iter()
        .any(|name| line.strip_prefix(name).is_some_and(|rest| rest.starts_with('(') || rest.starts_with('`') || rest.starts_with("<")))
}

/// A protected file as read: its hash, and its tests and skips (none for a file too large to count).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileMark {
    pub hash: String,
    pub tests: usize,
    pub skips: usize,
}

/// `bytes` of `path` read for the guard.
pub fn mark(path: &str, bytes: &[u8]) -> FileMark {
    let hash: String = Sha256::digest(bytes).iter().take(12).map(|b| format!("{b:02x}")).collect();
    if !is_test_path(path) || bytes.len() > MAXIMUM_COUNTED_BYTES {
        return FileMark { hash, tests: 0, skips: 0 };
    }
    let text = String::from_utf8_lossy(bytes);
    FileMark { hash, tests: count_tests(&text), skips: count_skips(&text) }
}

/// What the guard locked at the loop's start: each protected file's mark.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct GuardBaseline {
    pub files: BTreeMap<String, FileMark>,
}

impl GuardBaseline {
    pub fn tests(&self) -> usize {
        self.files.values().map(|m| m.tests).sum()
    }

    pub fn skips(&self) -> usize {
        self.files.values().map(|m| m.skips).sum()
    }

    /// The guard after a turn: `now` is every protected file as it is now (new test files included; a file missing is deleted). A
    /// baseline file changed or gone is touched; fewer tests than at the start is `testsRemoved`; more skips is `skipAdded`.
    pub fn compare(&self, now: &BTreeMap<String, FileMark>) -> Guard {
        let touched: Vec<String> =
            self.files.iter().filter(|(path, m)| now.get(*path).is_none_or(|n| n.hash != m.hash)).map(|(p, _)| p.clone()).collect();
        let tests_now: usize = now.values().map(|m| m.tests).sum();
        let skips_now: usize = now.values().map(|m| m.skips).sum();
        let tests_at_start = self.tests();
        let skips_added = skips_now.saturating_sub(self.skips());
        let mut flags = Vec::new();
        if tests_now < tests_at_start {
            flags.push(GuardFlag::TestsRemoved);
        }
        if skips_added > 0 {
            flags.push(GuardFlag::SkipAdded);
        }
        if !touched.is_empty() {
            flags.push(GuardFlag::ProtectedChanged);
        }
        Guard { protected_files: self.files.len(), touched, tests_at_start, tests_now, skips_added, flags }
    }

    /// The guard at the start.
    pub fn initial(&self) -> Guard {
        Guard {
            protected_files: self.files.len(),
            touched: Vec::new(),
            tests_at_start: self.tests(),
            tests_now: self.tests(),
            skips_added: 0,
            flags: Vec::new(),
        }
    }
}

// ── the prompts ──────────────────────────────────────────────────────────────────────────────────────────────────────────

/// The words the prompts use, in the app's language (src-tauri fills them from `loop.host.prompt.*`).
#[derive(Debug, Clone, Default)]
pub struct PromptWords {
    /// "완료 기준 (앱이 매번 직접 확인합니다):"
    pub criteria_header: String,
    /// The rules: existing tests and verify files are locked, new tests are welcome, no skipping.
    pub rules: String,
    /// "반복 {n}/{max}: 아직 통과하지 못한 기준이 있습니다. 고쳐 주세요."
    pub repair_header: String,
    /// "보호된 파일을 바꿨습니다: {files}. 되돌리고 다른 방법으로 고쳐 주세요."
    pub guard_touched: String,
    /// "테스트 수가 줄었습니다 ({start} → {now})."
    pub guard_tests_removed: String,
    /// "건너뛰기(skip)가 {count}개 늘었습니다."
    pub guard_skip_added: String,
}

/// The goal turn: the person's goal, then the criteria the app will check and the rules.
pub fn goal_prompt(goal: &str, criteria: &[Criterion], words: &PromptWords) -> String {
    let mut out = goal.trim().to_string();
    out.push_str("\n\n");
    out.push_str(&words.criteria_header);
    for c in criteria {
        out.push_str(&format!("\n- {}", c.text));
        if let (CheckKind::Command, Some(command)) = (c.check.kind, c.check.command.as_deref()) {
            out.push_str(&format!(" (`{command}`)"));
        }
    }
    if !words.rules.is_empty() {
        out.push_str("\n\n");
        out.push_str(&words.rules);
    }
    out
}

/// The repair turn: the header with the iteration, the guard's findings, then the failing checks' summary.
pub fn repair_prompt(n: u32, max: u32, guard: &Guard, summary: &str, words: &PromptWords) -> String {
    let mut out = words.repair_header.replace("{n}", &n.to_string()).replace("{max}", &max.to_string());
    for flag in &guard.flags {
        let line = match flag {
            GuardFlag::ProtectedChanged => {
                let files = guard.touched.iter().take(10).cloned().collect::<Vec<_>>().join(", ");
                words.guard_touched.replace("{files}", &files)
            }
            GuardFlag::TestsRemoved => words
                .guard_tests_removed
                .replace("{start}", &guard.tests_at_start.to_string())
                .replace("{now}", &guard.tests_now.to_string()),
            GuardFlag::SkipAdded => words.guard_skip_added.replace("{count}", &guard.skips_added.to_string()),
        };
        out.push_str(&format!("\n- {line}"));
    }
    if !summary.is_empty() {
        out.push_str("\n\n");
        out.push_str(summary);
    }
    out
}

// ── the driver ───────────────────────────────────────────────────────────────────────────────────────────────────────────

/// How a turn of the pane ended.
#[derive(Debug, Clone, PartialEq)]
pub enum TurnOutcome {
    /// It ended well: check. `run_id` is the request, `cost_usd` what it cost (when the ledger knew).
    Done { run_id: Option<String>, cost_usd: Option<f64> },
    /// It failed, was cancelled or waits for the person.
    Ended { run_id: Option<String>, cost_usd: Option<f64> },
    /// The person sent something to the pane or stopped its run.
    Person,
    /// The person paused or stopped the loop while it ran.
    Interrupted,
    /// The pane closed.
    PaneClosed,
}

/// One criterion's check after a turn.
#[derive(Debug, Clone, PartialEq)]
pub struct CheckResult {
    pub id: String,
    /// None: not decided (a checklist item; a check that could not tell).
    pub passed: Option<bool>,
    /// The commands it ran (for the repair's summary).
    pub commands: Vec<CommandResult>,
    pub browser: Option<verify::BrowserReport>,
    pub summary: String,
    pub duration_ms: u64,
}

/// What a turn changed in the run folder (from the request's checkpoints).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Changes {
    pub files: u32,
    pub added: u32,
    pub removed: u32,
}

/// What the person asked of a running loop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Control {
    Go,
    Pause,
    Stop,
}

/// What the driver needs from the app.
pub trait LoopHost {
    /// Sends `prompt` to the pane as its next request and waits for the turn to end.
    fn turn(&mut self, prompt: String) -> impl Future<Output = TurnOutcome> + Send;
    /// Runs the criteria's checks in order.
    fn check(&mut self, criteria: &[Criterion]) -> impl Future<Output = Vec<CheckResult>> + Send;
    /// Reads the protected files and compares them with the start.
    fn guard(&mut self) -> impl Future<Output = Guard> + Send;
    /// What the newest turn changed.
    fn changes(&mut self, run_id: Option<&str>) -> impl Future<Output = Changes> + Send;
    /// What the person asked (read between steps).
    fn control(&mut self) -> Control;
    /// Whether one more turn fits in today's budget.
    fn budget_allows(&mut self) -> bool;
    /// Minutes since the loop started (paused time included).
    fn elapsed_minutes(&mut self) -> f64;
    fn now(&mut self) -> String;
    /// The failure summary (secrets redacted) of the failing checks.
    fn summarize(&mut self, round: &RoundResult) -> String;
    /// The record changed: keep it and tell the page.
    fn update(&mut self, record: &LoopRecord);
}

/// Where [`drive`] starts.
#[derive(Debug, Clone, PartialEq)]
pub enum Start {
    /// A new loop: the goal turn first.
    Goal(String),
    /// A resumed one: check the folder as it is, then repair.
    Check,
}

/// The hash of a failed round: the failing criteria with their commands' failure hash, and the guard's flags. The same failure twice
/// (a repair that changed nothing that matters) gives the same hash.
pub fn round_hash(results: &[CheckResult], guard: &Guard) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"mighty-loop-round-v1\0");
    for r in results.iter().filter(|r| r.passed == Some(false)) {
        hasher.update(r.id.as_bytes());
        hasher.update(b"\0");
        let round = RoundResult { commands: r.commands.clone(), browser: r.browser.clone() };
        hasher.update(verify::failure_hash(&round).as_bytes());
    }
    for flag in &guard.flags {
        hasher.update(format!("{flag:?}").as_bytes());
    }
    for path in &guard.touched {
        hasher.update(path.as_bytes());
    }
    hasher.finalize().iter().take(8).map(|b| format!("{b:02x}")).collect()
}

/// Ends `record` in `state` for `reason`.
fn finish<H: LoopHost>(host: &mut H, record: &mut LoopRecord, state: LoopState, reason: Option<LoopStop>) {
    record.state = state;
    record.phase = LoopPhase::Done;
    record.stop_reason = reason;
    record.finished_at = Some(host.now());
    if let Some(last) = record.iterations.last_mut()
        && last.outcome == IterationOutcome::Running
    {
        last.outcome = if state == LoopState::Passed { IterationOutcome::Passed } else { IterationOutcome::Stopped };
        last.finished_at = record.finished_at.clone();
    }
    host.update(record);
}

/// Pauses `record` (the person's 잠시 멈춤): the current iteration ends as stopped.
fn pause<H: LoopHost>(host: &mut H, record: &mut LoopRecord) {
    record.state = LoopState::Paused;
    if let Some(last) = record.iterations.last_mut()
        && last.outcome == IterationOutcome::Running
    {
        last.outcome = IterationOutcome::Stopped;
        last.finished_at = Some(host.now());
    }
    host.update(record);
}

/// Runs the loop in `record` (already `running`, its criteria locked) from `start` until it passes, stops or is paused.
pub async fn drive<H: LoopHost>(host: &mut H, record: &mut LoopRecord, start: Start, words: &PromptWords) {
    let mut previous_hash: Option<String> = None;
    let mut prompt = match start {
        Start::Goal(prompt) => Some(prompt),
        Start::Check => None,
    };
    loop {
        if let Some(text) = prompt.take() {
            let n = record.iterations.len() as u32 + 1;
            let kind = if n == 1 { IterationKind::Build } else { IterationKind::Repair };
            record.phase = if kind == IterationKind::Build { LoopPhase::Build } else { LoopPhase::Repair };
            record.iterations.push(Iteration {
                n,
                kind,
                run_id: None,
                started_at: host.now(),
                finished_at: None,
                outcome: IterationOutcome::Running,
                failed: Vec::new(),
                changed_files: 0,
                added: 0,
                removed: 0,
                cost_usd: 0.0,
            });
            host.update(record);
            let outcome = host.turn(text).await;
            let (run_id, cost) = match &outcome {
                TurnOutcome::Done { run_id, cost_usd } | TurnOutcome::Ended { run_id, cost_usd } => (run_id.clone(), *cost_usd),
                _ => (None, None),
            };
            if let Some(last) = record.iterations.last_mut() {
                if run_id.is_some() {
                    last.run_id = run_id.clone();
                }
                last.cost_usd = cost.unwrap_or(0.0);
            }
            record.cost_usd += cost.unwrap_or(0.0);
            match outcome {
                TurnOutcome::Done { .. } => {}
                TurnOutcome::Ended { .. } => return finish(host, record, LoopState::Failed, Some(LoopStop::RepairEnded)),
                TurnOutcome::Person => return finish(host, record, LoopState::Stopped, Some(LoopStop::Person)),
                TurnOutcome::PaneClosed => return finish(host, record, LoopState::Stopped, Some(LoopStop::PaneClosed)),
                TurnOutcome::Interrupted => match host.control() {
                    Control::Pause => return pause(host, record),
                    _ => return finish(host, record, LoopState::Stopped, Some(LoopStop::Person)),
                },
            }
            let changes = host.changes(run_id.as_deref()).await;
            if let Some(last) = record.iterations.last_mut() {
                last.changed_files = changes.files;
                last.added = changes.added;
                last.removed = changes.removed;
            }
        }
        // Verify.
        if host.control() == Control::Stop {
            return finish(host, record, LoopState::Stopped, Some(LoopStop::Person));
        }
        record.phase = LoopPhase::Verify;
        host.update(record);
        let results = host.check(&record.criteria).await;
        let guard = host.guard().await;
        let at = host.now();
        for c in record.criteria.iter_mut() {
            let Some(result) = results.iter().find(|r| r.id == c.id) else { continue };
            match result.passed {
                Some(passed) => {
                    c.status = if passed { CriterionStatus::Passed } else { CriterionStatus::Failed };
                    c.evidence =
                        Some(Evidence { level: c.level, summary: result.summary.clone(), at: at.clone(), duration_ms: result.duration_ms });
                }
                None => c.status = CriterionStatus::Pending,
            }
        }
        record.guard = guard;
        let failed: Vec<String> = results.iter().filter(|r| r.passed == Some(false)).map(|r| r.id.clone()).collect();
        let passed = failed.is_empty()
            && record.guard.flags.is_empty()
            && record.criteria.iter().filter(|c| c.decides()).all(|c| c.status == CriterionStatus::Passed);
        if let Some(last) = record.iterations.last_mut()
            && last.outcome == IterationOutcome::Running
        {
            last.failed = failed.clone();
            last.outcome = if passed { IterationOutcome::Passed } else { IterationOutcome::Failed };
            last.finished_at = Some(at.clone());
        }
        if passed {
            return finish(host, record, LoopState::Passed, Some(LoopStop::Passed));
        }
        // A failure: stop, or ask for a repair.
        let hash = round_hash(&results, &record.guard);
        let stop = match host.control() {
            Control::Stop => Some((LoopState::Stopped, LoopStop::Person)),
            _ if previous_hash.as_deref() == Some(hash.as_str()) => Some((LoopState::Failed, LoopStop::SameFailure)),
            _ if record.iterations.len() as u32 >= record.limits.max_iterations => Some((LoopState::Failed, LoopStop::MaxIterations)),
            _ if host.elapsed_minutes() >= f64::from(record.limits.max_minutes) => Some((LoopState::Failed, LoopStop::MaxMinutes)),
            _ if record.cost_usd >= record.limits.max_cost_usd => Some((LoopState::Failed, LoopStop::MaxCost)),
            _ if !host.budget_allows() => Some((LoopState::Failed, LoopStop::Budget)),
            _ => None,
        };
        if let Some((state, reason)) = stop {
            return finish(host, record, state, Some(reason));
        }
        if host.control() == Control::Pause {
            return pause(host, record);
        }
        previous_hash = Some(hash);
        let round = RoundResult {
            commands: results.iter().filter(|r| r.passed == Some(false)).flat_map(|r| r.commands.clone()).collect(),
            browser: results.iter().filter(|r| r.passed == Some(false)).find_map(|r| r.browser.clone()),
        };
        let summary = host.summarize(&round);
        let n = record.iterations.len() as u32 + 1;
        prompt = Some(repair_prompt(n, record.limits.max_iterations, &record.guard, &summary, words));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    fn command(id: &str, cmd: &str) -> Criterion {
        let mut c = command_criterion(id, cmd);
        c.id = id.to_string();
        c
    }

    #[test]
    fn criteria_get_ids_and_need_a_command() {
        let checklist = Criterion {
            id: String::new(),
            text: "문서도 고친다".into(),
            check: Check { kind: CheckKind::Checklist, name: None, command: None, cwd: None },
            level: Level::L3,
            status: CriterionStatus::Passed,
            evidence: None,
        };
        assert_eq!(normalize_criteria(vec![checklist.clone()]), Err(CriteriaError::NoCommand));
        let out = normalize_criteria(vec![checklist, command_criterion("test", " npm test "), command_criterion("x", "y")]).unwrap();
        assert_eq!(out.iter().map(|c| c.id.as_str()).collect::<Vec<_>>(), ["c1", "c2", "c3"]);
        assert_eq!(out[0].level, Level::L1, "a checklist item is never more than L1");
        assert_eq!(out[0].status, CriterionStatus::Pending);
        assert_eq!(out[1].check.command.as_deref(), Some("npm test"));
        let mut empty = command_criterion("t", "");
        empty.check.command = Some("  ".into());
        assert_eq!(normalize_criteria(vec![empty]), Err(CriteriaError::Incomplete));
        let repeated = normalize_criteria(vec![command("a", "1"), command("a", "2")]).unwrap();
        assert_ne!(repeated[0].id, repeated[1].id);
    }

    #[test]
    fn the_hash_follows_what_is_checked_not_how_it_went() {
        let mut criteria = vec![command("c1", "npm test")];
        let before = criteria_hash(&criteria);
        criteria[0].status = CriterionStatus::Failed;
        criteria[0].evidence = Some(Evidence { level: Level::L2, summary: "1 failed".into(), at: "t".into(), duration_ms: 3 });
        assert_eq!(criteria_hash(&criteria), before);
        criteria[0].check.command = Some("npm test -- --passWithNoTests".into());
        assert_ne!(criteria_hash(&criteria), before);
        assert_eq!(before.len(), 16);
    }

    #[test]
    fn titles_and_limits() {
        assert_eq!(title_from_goal("\n\n  CI 초록불까지  \n자세히는…"), "CI 초록불까지");
        assert_eq!(title_from_goal(&"가".repeat(200)).chars().count(), MAXIMUM_TITLE_CHARS);
        let l = Limits { max_iterations: 0, max_minutes: 100_000, max_cost_usd: f64::NAN }.clamped();
        assert_eq!((l.max_iterations, l.max_minutes, l.max_cost_usd), (1, 1440, 5.0));
    }

    #[test]
    fn test_files_and_verify_files_are_protected() {
        for path in [
            "src/foo.test.ts",
            "src/Foo.spec.tsx",
            "pkg/foo_test.go",
            "tests/relay.rs",
            "app/__tests__/a.js",
            "test_api.py",
            "src/main/java/FooTest.java",
            "Tests/AppTests.swift",
            ".mighty.yaml",
            ".github/workflows/ci.yml",
        ] {
            assert!(is_protected(path), "{path}");
        }
        for path in ["src/latest.ts", "src/contest.rs", "README.md", "tests/fixtures/data.json", "src/attest.py"] {
            assert!(!is_protected(path), "{path}");
        }
    }

    #[test]
    fn tests_and_skips_are_counted() {
        let rust = "#[test]\nfn a() {}\n#[tokio::test]\nasync fn b() {}\n#[test]\n#[ignore]\nfn c() {}";
        assert_eq!((count_tests(rust), count_skips(rust)), (3, 1));
        let ts = "describe('x', () => {\n  it('a', () => {})\n  it.skip('b', () => {})\n  test.each`\n  xit('c', () => {})\n})";
        assert_eq!(count_tests(ts), 2, "it( and test.each`");
        assert_eq!(count_skips(ts), 2, ".skip( and xit(");
        let py = "def test_a():\n    pass\n@pytest.mark.skip\ndef test_b():\n    pytest.skip('no')";
        assert_eq!((count_tests(py), count_skips(py)), (2, 2));
    }

    #[test]
    fn the_guard_catches_a_changed_test_fewer_tests_and_a_new_skip() {
        let mut start = GuardBaseline::default();
        start.files.insert("a.test.ts".into(), mark("a.test.ts", b"it('a', () => {})\nit('b', () => {})"));
        start.files.insert(".mighty.yaml".into(), mark(".mighty.yaml", b"verify: {}"));
        assert_eq!(start.tests(), 2);
        let mut now = start.files.clone();
        assert!(start.compare(&now).flags.is_empty(), "nothing changed");
        now.insert("new.test.ts".into(), mark("new.test.ts", b"it('goal', () => {})"));
        let guard = start.compare(&now);
        assert!(guard.flags.is_empty(), "a new test is welcome: {guard:?}");
        assert_eq!(guard.tests_now, 3);
        now.insert("a.test.ts".into(), mark("a.test.ts", b"it('a', () => {})\nit.skip('b', () => {})"));
        let guard = start.compare(&now);
        assert_eq!(guard.touched, ["a.test.ts"]);
        assert!(guard.flags.contains(&GuardFlag::ProtectedChanged) && guard.flags.contains(&GuardFlag::SkipAdded));
        now.remove("new.test.ts");
        now.remove(".mighty.yaml");
        let guard = start.compare(&now);
        assert!(guard.flags.contains(&GuardFlag::TestsRemoved), "{guard:?}");
        assert_eq!(guard.touched, [".mighty.yaml", "a.test.ts"]);
    }

    #[test]
    fn a_running_loop_comes_back_paused() {
        let mut record = LoopRecord::draft("l".into(), "p", "w", "goal", vec![command("c1", "true")]);
        record.state = LoopState::Running;
        record.iterations.push(Iteration {
            n: 1,
            kind: IterationKind::Build,
            run_id: None,
            started_at: "t".into(),
            finished_at: None,
            outcome: IterationOutcome::Running,
            failed: vec![],
            changed_files: 0,
            added: 0,
            removed: 0,
            cost_usd: 0.0,
        });
        let back = record.after_restart();
        assert_eq!(back.state, LoopState::Paused);
        assert_eq!(back.stop_reason, None);
        assert_eq!(back.iterations[0].outcome, IterationOutcome::Stopped);
    }

    #[test]
    fn the_record_reads_back_as_the_contract_spells_it() {
        let mut record = LoopRecord::draft("loop-1".into(), "p", "w", "CI 초록불까지", vec![command("c1", "npm test")]);
        record.guard.flags.push(GuardFlag::SkipAdded);
        record.stop_reason = Some(LoopStop::MaxIterations);
        let value = serde_json::to_value(&record).unwrap();
        assert_eq!(value["state"], "draft");
        assert_eq!(value["phase"], "build");
        assert_eq!(value["criteria"][0]["check"]["kind"], "command");
        assert_eq!(value["criteria"][0]["level"], "L2");
        assert_eq!(value["limits"]["maxIterations"], 10);
        assert_eq!(value["guard"]["flags"][0], "skipAdded");
        assert_eq!(value["stopReason"], "maxIterations");
        assert!(value["finishedAt"].is_null() && value["baseCommit"].is_null());
        let back: LoopRecord = serde_json::from_value(value).unwrap();
        assert_eq!(back, record);
    }

    // ── the driver against a scripted host ──

    #[derive(Default)]
    struct Fake {
        turns: Vec<String>,
        outcomes: VecDeque<TurnOutcome>,
        checks: VecDeque<Vec<CheckResult>>,
        guards: VecDeque<Guard>,
        control: Vec<Control>,
        budget: bool,
        minutes: f64,
        updates: usize,
    }

    fn result(id: &str, passed: bool, tail: &str) -> CheckResult {
        CheckResult {
            id: id.into(),
            passed: Some(passed),
            commands: vec![CommandResult {
                name: id.into(),
                command: "x".into(),
                passed,
                exit_code: Some(i32::from(!passed)),
                timed_out: false,
                duration_ms: 1,
                output: tail.into(),
                error: None,
            }],
            browser: None,
            summary: if passed { "ok".into() } else { tail.into() },
            duration_ms: 1,
        }
    }

    impl LoopHost for Fake {
        async fn turn(&mut self, prompt: String) -> TurnOutcome {
            self.turns.push(prompt);
            self.outcomes.pop_front().unwrap_or(TurnOutcome::Done { run_id: Some(format!("r{}", self.turns.len())), cost_usd: Some(0.5) })
        }
        async fn check(&mut self, _criteria: &[Criterion]) -> Vec<CheckResult> {
            self.checks.pop_front().unwrap_or_default()
        }
        async fn guard(&mut self) -> Guard {
            self.guards.pop_front().unwrap_or_default()
        }
        async fn changes(&mut self, _run_id: Option<&str>) -> Changes {
            Changes { files: 2, added: 10, removed: 3 }
        }
        fn control(&mut self) -> Control {
            if self.control.len() > 1 { self.control.remove(0) } else { self.control.first().copied().unwrap_or(Control::Go) }
        }
        fn budget_allows(&mut self) -> bool {
            self.budget
        }
        fn elapsed_minutes(&mut self) -> f64 {
            self.minutes
        }
        fn now(&mut self) -> String {
            "2026-11-28T10:00:00Z".into()
        }
        fn summarize(&mut self, round: &RoundResult) -> String {
            verify::failure_summary(round, &|t| t.to_string())
        }
        fn update(&mut self, _record: &LoopRecord) {
            self.updates += 1;
        }
    }

    fn running(criteria: Vec<Criterion>) -> LoopRecord {
        let mut record = LoopRecord::draft("l".into(), "p", "w", "goal", criteria);
        record.criteria_hash = criteria_hash(&record.criteria);
        record.state = LoopState::Running;
        record
    }

    fn words() -> PromptWords {
        PromptWords {
            criteria_header: "Criteria:".into(),
            rules: "Rules.".into(),
            repair_header: "Round {n}/{max}".into(),
            guard_touched: "touched {files}".into(),
            guard_tests_removed: "tests {start}->{now}".into(),
            guard_skip_added: "skips +{count}".into(),
        }
    }

    fn block_on<F: Future>(f: F) -> F::Output {
        tokio::runtime::Builder::new_current_thread().build().unwrap().block_on(f)
    }

    #[test]
    fn a_loop_repairs_until_it_passes() {
        let mut host = Fake { budget: true, ..Fake::default() };
        host.checks.push_back(vec![result("c1", false, "1 failed: adds")]);
        host.checks.push_back(vec![result("c1", true, "")]);
        let mut record = running(vec![command("c1", "npm test")]);
        let goal = goal_prompt("make it add", &record.criteria, &words());
        assert!(goal.contains("make it add") && goal.contains("- c1 (`npm test`)") && goal.ends_with("Rules."));
        block_on(drive(&mut host, &mut record, Start::Goal(goal), &words()));
        assert_eq!(record.state, LoopState::Passed);
        assert_eq!(record.stop_reason, Some(LoopStop::Passed));
        assert_eq!(record.iterations.len(), 2);
        assert_eq!(record.iterations[0].kind, IterationKind::Build);
        assert_eq!(record.iterations[0].outcome, IterationOutcome::Failed);
        assert_eq!(record.iterations[0].failed, ["c1"]);
        assert_eq!(record.iterations[1].kind, IterationKind::Repair);
        assert_eq!(record.iterations[1].outcome, IterationOutcome::Passed);
        assert_eq!(record.iterations[1].changed_files, 2);
        assert!((record.cost_usd - 1.0).abs() < 1e-9);
        assert!(host.turns[1].starts_with("Round 2/10") && host.turns[1].contains("1 failed: adds"), "{}", host.turns[1]);
        assert_eq!(record.criteria[0].status, CriterionStatus::Passed);
        assert_eq!(record.criteria[0].evidence.as_ref().map(|e| e.level), Some(Level::L2));
    }

    #[test]
    fn a_passing_check_with_a_dirty_guard_does_not_pass() {
        let mut host = Fake { budget: true, ..Fake::default() };
        host.checks.push_back(vec![result("c1", true, "")]);
        host.checks.push_back(vec![result("c1", true, "")]);
        let dirty = Guard { touched: vec!["a.test.ts".into()], flags: vec![GuardFlag::ProtectedChanged], ..Guard::default() };
        host.guards.push_back(dirty);
        let mut record = running(vec![command("c1", "npm test")]);
        block_on(drive(&mut host, &mut record, Start::Goal("g".into()), &words()));
        assert_eq!(record.state, LoopState::Passed, "passes once the guard is clean again");
        assert_eq!(record.iterations.len(), 2);
        assert!(host.turns[1].contains("touched a.test.ts"), "{}", host.turns[1]);
    }

    #[test]
    fn the_same_failure_twice_stops() {
        let mut host = Fake { budget: true, ..Fake::default() };
        for _ in 0..3 {
            host.checks.push_back(vec![result("c1", false, "error at line 12")]);
        }
        let mut record = running(vec![command("c1", "npm test")]);
        block_on(drive(&mut host, &mut record, Start::Goal("g".into()), &words()));
        assert_eq!((record.state, record.stop_reason), (LoopState::Failed, Some(LoopStop::SameFailure)));
        assert_eq!(record.iterations.len(), 2);
    }

    #[test]
    fn limits_stop_it() {
        let mut host = Fake { budget: true, ..Fake::default() };
        for n in 0..5 {
            host.checks.push_back(vec![result("c1", false, &format!("failure kind {}", ["a", "b", "c", "d", "e"][n]))]);
        }
        let mut record = running(vec![command("c1", "t")]);
        record.limits.max_iterations = 3;
        block_on(drive(&mut host, &mut record, Start::Goal("g".into()), &words()));
        assert_eq!(record.stop_reason, Some(LoopStop::MaxIterations));
        assert_eq!(record.iterations.len(), 3);

        let mut host = Fake { budget: true, ..Fake::default() };
        host.checks.push_back(vec![result("c1", false, "x")]);
        let mut record = running(vec![command("c1", "t")]);
        record.limits.max_cost_usd = 0.4;
        block_on(drive(&mut host, &mut record, Start::Goal("g".into()), &words()));
        assert_eq!(record.stop_reason, Some(LoopStop::MaxCost));

        let mut host = Fake { budget: false, ..Fake::default() };
        host.checks.push_back(vec![result("c1", false, "x")]);
        let mut record = running(vec![command("c1", "t")]);
        block_on(drive(&mut host, &mut record, Start::Goal("g".into()), &words()));
        assert_eq!(record.stop_reason, Some(LoopStop::Budget));

        let mut host = Fake { budget: true, minutes: 500.0, ..Fake::default() };
        host.checks.push_back(vec![result("c1", false, "x")]);
        let mut record = running(vec![command("c1", "t")]);
        block_on(drive(&mut host, &mut record, Start::Goal("g".into()), &words()));
        assert_eq!(record.stop_reason, Some(LoopStop::MaxMinutes));
    }

    #[test]
    fn the_person_and_a_bad_turn_end_it() {
        let mut host = Fake { budget: true, ..Fake::default() };
        host.outcomes.push_back(TurnOutcome::Person);
        let mut record = running(vec![command("c1", "t")]);
        block_on(drive(&mut host, &mut record, Start::Goal("g".into()), &words()));
        assert_eq!((record.state, record.stop_reason), (LoopState::Stopped, Some(LoopStop::Person)));
        assert_eq!(record.iterations[0].outcome, IterationOutcome::Stopped);

        let mut host = Fake { budget: true, ..Fake::default() };
        host.outcomes.push_back(TurnOutcome::Ended { run_id: Some("r1".into()), cost_usd: Some(0.2) });
        let mut record = running(vec![command("c1", "t")]);
        block_on(drive(&mut host, &mut record, Start::Goal("g".into()), &words()));
        assert_eq!((record.state, record.stop_reason), (LoopState::Failed, Some(LoopStop::RepairEnded)));
        assert_eq!(record.iterations[0].run_id.as_deref(), Some("r1"));
    }

    #[test]
    fn a_pause_waits_after_the_check_and_a_resume_checks_first() {
        let mut host = Fake { budget: true, control: vec![Control::Go, Control::Go, Control::Pause], ..Fake::default() };
        host.checks.push_back(vec![result("c1", false, "x")]);
        let mut record = running(vec![command("c1", "t")]);
        block_on(drive(&mut host, &mut record, Start::Goal("g".into()), &words()));
        assert_eq!(record.state, LoopState::Paused);
        assert_eq!(record.stop_reason, None);
        assert_eq!(host.turns.len(), 1, "no repair was sent");

        record.state = LoopState::Running;
        let mut host = Fake { budget: true, ..Fake::default() };
        host.checks.push_back(vec![result("c1", true, "")]);
        block_on(drive(&mut host, &mut record, Start::Check, &words()));
        assert_eq!(record.state, LoopState::Passed);
        assert!(host.turns.is_empty(), "the folder already passed");
    }

    #[test]
    fn a_checklist_item_never_passes_a_loop_alone_nor_blocks_it() {
        let mut host = Fake { budget: true, ..Fake::default() };
        let mut undecided = result("c2", true, "");
        undecided.passed = None;
        host.checks.push_back(vec![result("c1", true, ""), undecided]);
        let mut checklist = command("c2", "x");
        checklist.check = Check { kind: CheckKind::Checklist, name: None, command: None, cwd: None };
        let mut record = running(vec![command("c1", "t"), checklist]);
        block_on(drive(&mut host, &mut record, Start::Goal("g".into()), &words()));
        assert_eq!(record.state, LoopState::Passed);
        assert_eq!(record.criteria[1].status, CriterionStatus::Pending);
    }
}
