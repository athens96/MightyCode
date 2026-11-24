//! The verification gate after an agent's turn (`MC-48`): what one verification round found ([`RoundResult`]), the failure summary
//! handed back to the agent ([`failure_summary`]), the hash that tells a repeated failure ([`failure_hash`]), the loop that verifies,
//! asks the same agent to repair and verifies again ([`run_loop`]) with its stops, and the receipt the result card shows
//! ([`VerifyReceipt`]). The browser check's rules live here too: which addresses it may visit ([`allowed_browser_url`]), where a
//! Chromium-family browser is installed ([`browser_candidates`]) and how the DevTools events of a page are counted ([`PageFindings`]).
//! The host (src-tauri `verify/`) runs the commands, the browser and the follow-up turns.

use std::future::Future;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest as _, Sha256};

/// The most characters of the failure summary handed to the agent.
pub const MAXIMUM_SUMMARY_CHARS: usize = 6_000;
/// The lines kept from the end of each failing command's output.
pub const SUMMARY_TAIL_LINES: usize = 40;
/// The most characters of one command's output kept on the receipt.
pub const RECEIPT_OUTPUT_CHARS: usize = 4_000;

// ── one round ───────────────────────────────────────────────────────────────────────────────────────────────────────────

/// How one verify command ended.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommandResult {
    pub name: String,
    pub command: String,
    pub passed: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    #[serde(default)]
    pub timed_out: bool,
    pub duration_ms: u64,
    /// The end of its output (redacted, at most [`RECEIPT_OUTPUT_CHARS`]).
    #[serde(default)]
    pub output: String,
    /// Why it could not start.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// What the browser saw on one page.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PageFindings {
    pub url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<u16>,
    #[serde(default)]
    pub loaded: bool,
    #[serde(default)]
    pub console_errors: Vec<String>,
    #[serde(default)]
    pub exceptions: Vec<String>,
    /// `<status or error> <method> <url>` of each failed request (4xx/5xx or a network error).
    #[serde(default)]
    pub failed_requests: Vec<String>,
    /// The accessibility tree, trimmed: `role "name"` lines.
    #[serde(default)]
    pub accessibility: Vec<String>,
    #[serde(default)]
    pub accessibility_nodes: usize,
    /// Buttons, links, images and fields without an accessible name.
    #[serde(default)]
    pub unnamed_controls: Vec<String>,
    /// The screenshot's file name in the app's screenshot folder.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub screenshot: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl PageFindings {
    pub fn problems(&self) -> usize {
        self.console_errors.len() + self.exceptions.len() + self.failed_requests.len() + usize::from(self.error.is_some())
    }
}

/// The browser check of one round: the pages, or why it did not run (`unavailable`: no browser installed, no address).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserReport {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub browser: Option<String>,
    #[serde(default)]
    pub pages: Vec<PageFindings>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unavailable: Option<String>,
}

impl BrowserReport {
    pub fn unavailable(reason: impl Into<String>) -> Self {
        BrowserReport { browser: None, pages: Vec::new(), unavailable: Some(reason.into()) }
    }

    /// Whether the check found something to fix: a page error, a console error, an exception or a failed request. A browser that
    /// is not there is no failure (the rest of the verification still decides).
    pub fn failed(&self) -> bool {
        self.pages.iter().any(|page| page.problems() > 0)
    }

    pub fn counts(&self) -> BrowserCounts {
        let mut counts = BrowserCounts { pages: self.pages.len(), ..BrowserCounts::default() };
        for page in &self.pages {
            counts.console_errors += page.console_errors.len();
            counts.exceptions += page.exceptions.len();
            counts.failed_requests += page.failed_requests.len();
            counts.unnamed_controls += page.unnamed_controls.len();
            counts.page_errors += usize::from(page.error.is_some());
        }
        counts
    }
}

/// The browser findings counted, for the card and the phone.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserCounts {
    pub pages: usize,
    pub console_errors: usize,
    pub exceptions: usize,
    pub failed_requests: usize,
    pub unnamed_controls: usize,
    pub page_errors: usize,
}

/// What one verification round found.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RoundResult {
    pub commands: Vec<CommandResult>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub browser: Option<BrowserReport>,
}

impl RoundResult {
    pub fn passed(&self) -> bool {
        self.commands.iter().all(|c| c.passed) && !self.browser.as_ref().is_some_and(BrowserReport::failed)
    }
}

// ── the failure summary and its hash ────────────────────────────────────────────────────────────────────────────────────

/// The last `lines` lines of `text`, ANSI and control characters gone.
fn tail_lines(text: &str, lines: usize) -> String {
    let clean = strip_ansi(text);
    let all: Vec<&str> = clean.lines().collect();
    let from = all.len().saturating_sub(lines);
    all[from..].join("\n")
}

/// `text` without ANSI escape sequences and control characters other than line breaks and tabs.
pub fn strip_ansi(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            match chars.peek() {
                Some('[') => {
                    chars.next();
                    for next in chars.by_ref() {
                        if ('@'..='~').contains(&next) {
                            break;
                        }
                    }
                }
                Some(']') => {
                    chars.next();
                    while let Some(next) = chars.next() {
                        if next == '\u{7}' {
                            break;
                        }
                        if next == '\u{1b}' && chars.peek() == Some(&'\\') {
                            chars.next();
                            break;
                        }
                    }
                }
                _ => {
                    chars.next();
                }
            }
            continue;
        }
        if c == '\r' {
            continue;
        }
        if c.is_control() && c != '\n' && c != '\t' {
            continue;
        }
        out.push(c);
    }
    out
}

/// The first `max` characters of `text`, with `…` when cut.
pub fn cap_chars(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let mut out: String = text.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

/// The end of `text`, at most `max` characters, with `…` in front when cut.
pub fn cap_chars_tail(text: &str, max: usize) -> String {
    let count = text.chars().count();
    if count <= max {
        return text.to_string();
    }
    let mut out = String::from("…");
    out.extend(text.chars().skip(count - max.saturating_sub(1)));
    out
}

/// The summary of what failed in `round`, for the agent: each failing command's name, how it ended and the last
/// [`SUMMARY_TAIL_LINES`] lines of its output, then the browser's findings; every part through `redact` (the host's secret
/// redactor) and the whole at most [`MAXIMUM_SUMMARY_CHARS`] characters. Each command gets an equal share, so a long first log
/// never hides the second failure.
pub fn failure_summary(round: &RoundResult, redact: &dyn Fn(&str) -> String) -> String {
    let failing: Vec<&CommandResult> = round.commands.iter().filter(|c| !c.passed).collect();
    let browser_part = round.browser.as_ref().filter(|b| b.failed()).map(|b| browser_summary(b, redact));
    let parts = failing.len() + usize::from(browser_part.is_some());
    if parts == 0 {
        return String::new();
    }
    let share = (MAXIMUM_SUMMARY_CHARS / parts).saturating_sub(16).max(200);
    let mut sections = Vec::new();
    for command in failing {
        let how = if command.timed_out {
            "timed out".to_string()
        } else if let Some(error) = &command.error {
            format!("could not start: {}", redact(error))
        } else {
            format!("exit code {}", command.exit_code.map_or_else(|| "?".into(), |c| c.to_string()))
        };
        let head = format!("### {} — {how}\n$ {}\n", command.name, redact(&command.command));
        let tail = redact(&tail_lines(&command.output, SUMMARY_TAIL_LINES));
        let body = cap_chars_tail(tail.trim_end(), share.saturating_sub(head.chars().count()).max(80));
        sections.push(if body.is_empty() { head.trim_end().to_string() } else { format!("{head}```\n{body}\n```") });
    }
    if let Some(browser) = browser_part {
        sections.push(cap_chars(&browser, share));
    }
    cap_chars(&sections.join("\n\n"), MAXIMUM_SUMMARY_CHARS)
}

fn browser_summary(report: &BrowserReport, redact: &dyn Fn(&str) -> String) -> String {
    let mut out = String::from("### browser");
    for page in report.pages.iter().filter(|p| p.problems() > 0) {
        out.push_str(&format!("\n{}", page.url));
        if let Some(error) = &page.error {
            out.push_str(&format!("\n- page: {}", redact(error)));
        }
        for (label, items) in
            [("console error", &page.console_errors), ("uncaught exception", &page.exceptions), ("failed request", &page.failed_requests)]
        {
            for item in items.iter().take(10) {
                out.push_str(&format!("\n- {label}: {}", cap_chars(&redact(item), 300)));
            }
            if items.len() > 10 {
                out.push_str(&format!("\n- … {} more {label}s", items.len() - 10));
            }
        }
    }
    out
}

/// A failure's text with what changes from run to run taken out: numbers (times, line counts, ports, ids) read as `0`, hex ids of 8 or
/// more characters as `#`, spaces collapsed, lower case.
pub fn normalize_failure_text(text: &str) -> String {
    let clean = strip_ansi(text).to_lowercase();
    let mut out = String::with_capacity(clean.len());
    let mut token = String::new();
    let flush = |token: &mut String, out: &mut String| {
        if token.is_empty() {
            return;
        }
        let hexish = token.len() >= 8 && token.chars().all(|c| c.is_ascii_hexdigit()) && token.chars().any(|c| c.is_ascii_digit());
        if hexish {
            out.push('#');
        } else {
            let mut last_digit = false;
            for c in token.chars() {
                if c.is_ascii_digit() {
                    if !last_digit {
                        out.push('0');
                    }
                    last_digit = true;
                } else {
                    out.push(c);
                    last_digit = false;
                }
            }
        }
        token.clear();
    };
    let mut last_space = true;
    for c in clean.chars() {
        if c.is_alphanumeric() {
            token.push(c);
            last_space = false;
        } else {
            flush(&mut token, &mut out);
            if c.is_whitespace() {
                if !last_space {
                    out.push(' ');
                }
                last_space = true;
            } else {
                out.push(c);
                last_space = false;
            }
        }
    }
    flush(&mut token, &mut out);
    out.trim().to_string()
}

/// The hash of `round`'s failure: which commands failed and how, the normalized end of their output, and the browser's findings.
/// The same failure twice (a repair that changed nothing that matters) gives the same hash.
pub fn failure_hash(round: &RoundResult) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"mighty-verify-failure-v1\0");
    for command in round.commands.iter().filter(|c| !c.passed) {
        hasher.update(command.name.as_bytes());
        hasher.update(b"\0");
        hasher.update(if command.timed_out { b"timeout".as_slice() } else { b"exit".as_slice() });
        hasher.update(command.exit_code.unwrap_or(-1).to_le_bytes());
        hasher.update(normalize_failure_text(&tail_lines(&command.output, SUMMARY_TAIL_LINES)).as_bytes());
        hasher.update(b"\0");
    }
    if let Some(browser) = round.browser.as_ref().filter(|b| b.failed()) {
        for page in &browser.pages {
            hasher.update(page.url.as_bytes());
            for item in page.console_errors.iter().chain(&page.exceptions).chain(&page.failed_requests).chain(&page.error) {
                hasher.update(normalize_failure_text(item).as_bytes());
                hasher.update(b"\0");
            }
        }
    }
    let digest = hasher.finalize();
    digest.iter().take(16).map(|b| format!("{b:02x}")).collect()
}

// ── the receipt ─────────────────────────────────────────────────────────────────────────────────────────────────────────

/// Where a verification stands or how it ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum VerifyStatus {
    /// Commands are running.
    Running,
    /// The agent was asked to repair and is working.
    Repairing,
    Passed,
    Failed,
}

/// Why the loop stopped before a pass.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum StopReason {
    /// `verify.maxRounds` repairs were asked for and the last verification still failed.
    MaxRounds,
    /// The failure was the same as the round before.
    SameFailure,
    /// Another repair would go past the daily budget.
    Budget,
    /// The person sent something or stopped the run.
    Person,
    /// Automatic repair is off for the pane (or `maxRounds: 0`, or a manual verification).
    RepairOff,
    /// The repair turn did not end well (failed, cancelled, waiting for the person).
    RepairEnded,
}

/// The receipt of a verification, on the result card and the phone.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VerifyReceipt {
    pub id: String,
    pub status: VerifyStatus,
    /// Verification rounds run (the first and one after each repair).
    pub rounds: u32,
    /// Repair requests sent to the agent.
    pub repairs: u32,
    pub max_rounds: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop_reason: Option<StopReason>,
    /// The last round's commands.
    pub commands: Vec<CommandResult>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub browser: Option<BrowserReport>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub browser_counts: Option<BrowserCounts>,
    /// The short hash of the approved `.mighty.yaml` it ran under.
    pub config_hash: String,
    pub started_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finished_at: Option<String>,
    /// Whether the person started it from the card.
    #[serde(default)]
    pub manual: bool,
    /// The requests it followed: the turn that was verified, then each repair's (the card shows it on the last).
    #[serde(default)]
    pub run_ids: Vec<String>,
}

impl VerifyReceipt {
    pub fn new(id: impl Into<String>, config_hash: impl Into<String>, max_rounds: u32, started_at: impl Into<String>) -> Self {
        VerifyReceipt {
            id: id.into(),
            status: VerifyStatus::Running,
            rounds: 0,
            repairs: 0,
            max_rounds,
            stop_reason: None,
            commands: Vec::new(),
            browser: None,
            browser_counts: None,
            config_hash: config_hash.into(),
            started_at: started_at.into(),
            finished_at: None,
            manual: false,
            run_ids: Vec::new(),
        }
    }

    /// The receipt after `round`.
    pub fn record(&mut self, round: &RoundResult) {
        self.rounds += 1;
        self.commands = round.commands.clone();
        self.browser_counts = round.browser.as_ref().map(BrowserReport::counts);
        self.browser = round.browser.clone();
    }

    pub fn finished(&self) -> bool {
        matches!(self.status, VerifyStatus::Passed | VerifyStatus::Failed)
    }
}

// ── the loop ────────────────────────────────────────────────────────────────────────────────────────────────────────────

/// How a repair turn ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RepairOutcome {
    /// The agent's turn ended well: verify again.
    Done,
    /// The turn failed, was cancelled or waits for the person.
    Ended,
    /// The person sent something or stopped it.
    Person,
}

/// What the loop needs from the host: run a round, know whether the person stepped in or the budget is spent, send the repair
/// request to the same agent and wait for its turn to end, and show the receipt as it changes.
pub trait VerifyHost {
    fn verify(&mut self) -> impl Future<Output = RoundResult> + Send;
    /// Whether the person sent anything to the pane or stopped its run since the loop began.
    fn person_stepped_in(&mut self) -> bool;
    /// Whether one more agent turn fits in today's budget.
    fn budget_allows(&mut self) -> bool;
    fn repair(&mut self, prompt: String) -> impl Future<Output = RepairOutcome> + Send;
    fn update(&mut self, receipt: &VerifyReceipt);
}

/// The loop's settings.
#[derive(Debug, Clone)]
pub struct LoopSettings {
    /// Repairs allowed (0: verify once and report).
    pub max_rounds: u32,
    /// Whether the agent is asked to repair at all (the pane's toggle; off for a manual verification).
    pub auto_repair: bool,
    /// The header put before the failure summary ("검증 실패 — 고쳐 주세요"), with `{round}` and `{max}`.
    pub repair_header: String,
}

/// The repair request: the header (its `{round}` and `{max}` filled) and the summary.
pub fn repair_prompt(header: &str, round: u32, max: u32, summary: &str) -> String {
    let head = header.replace("{round}", &round.to_string()).replace("{max}", &max.to_string());
    format!("{head}\n\n{summary}")
}

/// Verifies; while it fails, asks the agent to repair and verifies again. Stops at a pass; after `max_rounds` repairs; when the
/// failure is the same as the round before; when another turn would go past the budget; when the person steps in; when a repair turn
/// does not end well; at once when automatic repair is off. The receipt is passed to `host.update` after each step.
pub async fn run_loop<H: VerifyHost>(
    host: &mut H,
    receipt: &mut VerifyReceipt,
    settings: &LoopSettings,
    redact: &(dyn Fn(&str) -> String + Sync),
) {
    let mut previous_hash: Option<String> = None;
    loop {
        receipt.status = VerifyStatus::Running;
        host.update(receipt);
        let round = host.verify().await;
        receipt.record(&round);
        if round.passed() {
            receipt.status = VerifyStatus::Passed;
            receipt.stop_reason = None;
            host.update(receipt);
            return;
        }
        let hash = failure_hash(&round);
        let stop = if !settings.auto_repair || settings.max_rounds == 0 {
            Some(StopReason::RepairOff)
        } else if host.person_stepped_in() {
            Some(StopReason::Person)
        } else if previous_hash.as_deref() == Some(hash.as_str()) {
            Some(StopReason::SameFailure)
        } else if receipt.repairs >= settings.max_rounds {
            Some(StopReason::MaxRounds)
        } else if !host.budget_allows() {
            Some(StopReason::Budget)
        } else {
            None
        };
        if let Some(reason) = stop {
            receipt.status = VerifyStatus::Failed;
            receipt.stop_reason = Some(reason);
            host.update(receipt);
            return;
        }
        previous_hash = Some(hash);
        receipt.repairs += 1;
        receipt.status = VerifyStatus::Repairing;
        host.update(receipt);
        let prompt = repair_prompt(&settings.repair_header, receipt.repairs, settings.max_rounds, &failure_summary(&round, redact));
        match host.repair(prompt).await {
            RepairOutcome::Done if !host.person_stepped_in() => {}
            RepairOutcome::Done | RepairOutcome::Person => {
                receipt.status = VerifyStatus::Failed;
                receipt.stop_reason = Some(StopReason::Person);
                host.update(receipt);
                return;
            }
            RepairOutcome::Ended => {
                receipt.status = VerifyStatus::Failed;
                receipt.stop_reason = Some(StopReason::RepairEnded);
                host.update(receipt);
                return;
            }
        }
    }
}

// ── the browser check's rules ───────────────────────────────────────────────────────────────────────────────────────────

/// The addresses the browser check may visit: the worktree's port block (`base` .. `base + count - 1`) and the port of the address
/// a run action printed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BrowserScope {
    pub block: Option<(u16, u16)>,
    pub detected: Option<String>,
}

impl BrowserScope {
    fn port_allowed(&self, port: u16) -> bool {
        let in_block =
            self.block.is_some_and(|(base, count)| count > 0 && port >= base && u32::from(port) < u32::from(base) + u32::from(count));
        let detected = self.detected.as_deref().and_then(|url| url::Url::parse(url).ok()).and_then(|u| u.port_or_known_default());
        in_block || detected == Some(port)
    }
}

/// `url` checked for the browser: `http(s)` on `localhost`, `127.0.0.1` or `[::1]`, no credentials, and a port in `scope`. Err with
/// the reason otherwise.
pub fn allowed_browser_url(url: &str, scope: &BrowserScope) -> Result<url::Url, String> {
    let parsed = url::Url::parse(url.trim()).map_err(|_| format!("'{url}' is not an address."))?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err(format!("'{url}': only http addresses on this computer are checked."));
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err(format!("'{url}': an address with a user name or password is not checked."));
    }
    if !matches!(parsed.host_str(), Some("localhost" | "127.0.0.1" | "[::1]")) {
        return Err(format!("'{url}': only localhost and 127.0.0.1 are checked."));
    }
    let Some(port) = parsed.port_or_known_default() else { return Err(format!("'{url}' names no port.")) };
    if !scope.port_allowed(port) {
        return Err(format!("'{url}': port {port} is not this worktree's (its port block or its run action's address)."));
    }
    Ok(parsed)
}

/// The addresses to visit: each of `paths` (default `/`) on `base`, all checked with [`allowed_browser_url`].
pub fn browser_targets(base: &str, paths: &[String], scope: &BrowserScope) -> Result<Vec<url::Url>, String> {
    let base = allowed_browser_url(base, scope)?;
    let paths: Vec<String> = if paths.is_empty() { vec!["/".into()] } else { paths.to_vec() };
    paths
        .iter()
        .take(crate::mighty_yaml::MAXIMUM_VERIFY_PATHS)
        .map(|path| {
            if !path.starts_with('/') || path.starts_with("//") {
                return Err(format!("'{path}' must start with a single /."));
            }
            let joined = base.join(path).map_err(|_| format!("'{path}' is not a path."))?;
            allowed_browser_url(joined.as_str(), scope)
        })
        .collect()
}

/// Where a Chromium-family browser (Chrome, Edge, Chromium, Brave) is installed, in the order they are tried, for the OS named
/// (`macos`, `windows`, anything else is Linux). `home` and the Windows folders (`ProgramFiles`, `ProgramFiles(x86)`,
/// `LocalAppData`) come from the host. Never downloaded.
pub fn browser_candidates(os: &str, home: Option<&Path>, windows_dirs: &[PathBuf]) -> Vec<(String, PathBuf)> {
    let mut out = Vec::new();
    match os {
        "macos" => {
            let apps = [
                ("Google Chrome", "Google Chrome.app/Contents/MacOS/Google Chrome"),
                ("Microsoft Edge", "Microsoft Edge.app/Contents/MacOS/Microsoft Edge"),
                ("Chromium", "Chromium.app/Contents/MacOS/Chromium"),
                ("Brave", "Brave Browser.app/Contents/MacOS/Brave Browser"),
            ];
            let mut roots = vec![PathBuf::from("/Applications")];
            if let Some(home) = home {
                roots.push(home.join("Applications"));
            }
            for (name, rel) in apps {
                for root in &roots {
                    out.push((name.to_string(), root.join(rel)));
                }
            }
        }
        "windows" => {
            let apps = [
                ("Google Chrome", r"Google\Chrome\Application\chrome.exe"),
                ("Microsoft Edge", r"Microsoft\Edge\Application\msedge.exe"),
                ("Chromium", r"Chromium\Application\chrome.exe"),
                ("Brave", r"BraveSoftware\Brave-Browser\Application\brave.exe"),
            ];
            for (name, rel) in apps {
                for root in windows_dirs {
                    out.push((name.to_string(), root.join(rel)));
                }
            }
        }
        _ => {
            for (name, path) in [
                ("Google Chrome", "/usr/bin/google-chrome"),
                ("Google Chrome", "/usr/bin/google-chrome-stable"),
                ("Microsoft Edge", "/usr/bin/microsoft-edge"),
                ("Chromium", "/usr/bin/chromium"),
                ("Chromium", "/usr/bin/chromium-browser"),
                ("Chromium", "/snap/bin/chromium"),
                ("Brave", "/usr/bin/brave-browser"),
            ] {
                out.push((name.to_string(), PathBuf::from(path)));
            }
        }
    }
    out
}

/// The first of `candidates` for which `exists` holds.
pub fn find_browser(candidates: &[(String, PathBuf)], exists: impl Fn(&Path) -> bool) -> Option<(String, PathBuf)> {
    candidates.iter().find(|(_, path)| exists(path)).cloned()
}

/// The most lines kept of the accessibility tree, and of each kind of finding.
pub const ACCESSIBILITY_LINES: usize = 60;
pub const FINDINGS_PER_KIND: usize = 50;

fn push_capped(list: &mut Vec<String>, text: String) {
    if list.len() < FINDINGS_PER_KIND {
        list.push(cap_chars(&text, 500));
    }
}

impl PageFindings {
    /// Counts one DevTools event of the page (`Runtime.consoleAPICalled`, `Runtime.exceptionThrown`, `Log.entryAdded`,
    /// `Network.requestWillBeSent`, `Network.responseReceived`, `Network.loadingFailed`); `requests` keeps each request's method
    /// and address by id.
    pub fn observe(&mut self, method: &str, params: &Value, requests: &mut std::collections::HashMap<String, (String, String)>) {
        let text = |v: &Value| v.as_str().unwrap_or_default().to_string();
        match method {
            "Runtime.consoleAPICalled" if params["type"] == "error" || params["type"] == "assert" => {
                let args: Vec<String> = params["args"]
                    .as_array()
                    .map(|args| {
                        args.iter()
                            .map(|a| {
                                a["value"]
                                    .as_str()
                                    .map(str::to_string)
                                    .or_else(|| a["description"].as_str().map(str::to_string))
                                    .unwrap_or_else(|| a["value"].to_string())
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                push_capped(&mut self.console_errors, args.join(" "));
            }
            "Runtime.exceptionThrown" => {
                let details = &params["exceptionDetails"];
                let description =
                    details["exception"]["description"].as_str().map(str::to_string).unwrap_or_else(|| text(&details["text"]));
                push_capped(&mut self.exceptions, description);
            }
            "Log.entryAdded" if params["entry"]["level"] == "error" => {
                let entry = &params["entry"];
                // A failed request is counted once, from the network events.
                if entry["source"] != "network" {
                    push_capped(&mut self.console_errors, text(&entry["text"]));
                }
            }
            "Network.requestWillBeSent" => {
                let id = text(&params["requestId"]);
                requests.insert(id, (text(&params["request"]["method"]), text(&params["request"]["url"])));
            }
            "Network.responseReceived" => {
                let status = params["response"]["status"].as_u64().unwrap_or(0);
                let url = text(&params["response"]["url"]);
                if params["type"] == "Document" && self.status.is_none() && url.trim_end_matches('/') == self.url.trim_end_matches('/') {
                    self.status = u16::try_from(status).ok();
                }
                if status >= 400 {
                    let method = requests.get(&text(&params["requestId"])).map_or("GET".to_string(), |(m, _)| m.clone());
                    push_capped(&mut self.failed_requests, format!("{status} {method} {url}"));
                }
            }
            "Network.loadingFailed" if params["canceled"] != true => {
                let (method, url) = requests.get(&text(&params["requestId"])).cloned().unwrap_or_else(|| ("GET".into(), String::new()));
                push_capped(&mut self.failed_requests, format!("{} {method} {url}", text(&params["errorText"])));
            }
            _ => {}
        }
    }

    /// The accessibility tree (`Accessibility.getFullAXTree`'s `nodes`) trimmed: `role "name"` lines of the nodes that are not
    /// ignored, indented by depth, at most [`ACCESSIBILITY_LINES`]; and the controls without a name.
    pub fn read_accessibility(&mut self, nodes: &[Value]) {
        let by_id: std::collections::HashMap<&str, &Value> = nodes.iter().filter_map(|n| n["nodeId"].as_str().map(|id| (id, n))).collect();
        let mut depth: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
        let mut kept = 0usize;
        for node in nodes {
            let id = node["nodeId"].as_str().unwrap_or_default();
            let parent_depth = node["parentId"].as_str().and_then(|p| depth.get(p).copied());
            let ignored = node["ignored"] == true;
            let here = parent_depth.map_or(0, |d| if ignored { d } else { d + 1 });
            depth.insert(id, here);
            if ignored {
                continue;
            }
            kept += 1;
            let role = node["role"]["value"].as_str().unwrap_or_default();
            let name = node["name"]["value"].as_str().unwrap_or_default().trim();
            if matches!(role, "none" | "generic" | "InlineTextBox" | "StaticText" | "LineBreak") && name.is_empty() {
                continue;
            }
            if matches!(role, "button" | "link" | "image" | "img" | "textbox" | "combobox" | "checkbox" | "searchbox") && name.is_empty() {
                push_capped(&mut self.unnamed_controls, role.to_string());
            }
            if self.accessibility.len() < ACCESSIBILITY_LINES && !matches!(role, "StaticText" | "InlineTextBox") {
                let indent = "  ".repeat(here.min(8));
                let line = if name.is_empty() { format!("{indent}{role}") } else { format!("{indent}{role} \"{}\"", cap_chars(name, 80)) };
                self.accessibility.push(line);
            }
        }
        let _ = by_id;
        self.accessibility_nodes = kept;
    }
}
