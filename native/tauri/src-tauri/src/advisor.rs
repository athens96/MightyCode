//! MC-28: another agent's opinion on a pane's work (`/advisor [agent] [question]`, and `/committee [agent agent] [question]` for two
//! side by side). The pane's last plan (if any), last result, a capped and redacted summary of the files its last request changed and
//! the person's question go to a different agent, run once in a read-only mode: Claude in plan mode with only Read, Grep and Glob and
//! no session saved; Codex's `exec` in its read-only sandbox, never asking, nothing saved; Antigravity in plan mode in its sandbox; pi
//! with only its read tools, no MCP, no session. opencode and Hermes are not offered: their one-shot runs approve what they ask on
//! their own. [`read_only`] checks every argument list before it runs. The answer lands as an opinion card on the pane
//! (`RunSession.opinions`: agent, model, cost when the CLI says it, body), 입력창에 넣기 on the page puts it into the composer, and its
//! cost goes into the usage ledger under the pane's workspace. No timer of its own: one task per question, ended by the answer or by
//! [`TIME_LIMIT`].

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use mighty_core::model::{OPINION_BODY_LIMIT, OPINION_CARDS_KEPT, OpinionCard, RunSession};
use mighty_core::{time, wire};
use serde_json::Value;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::host::Host;

/// The agents an opinion may come from, in the order the first free one is picked.
pub const ADVISORS: [&str; 4] = ["claude", "codex", "gemini", "pi"];

/// How long an opinion may take.
pub const TIME_LIMIT: Duration = Duration::from_secs(600);

/// The most a prompt holds, in characters (Antigravity takes it as an argument, which Windows caps at 32,767).
pub const PROMPT_LIMIT: usize = 20_000;
const PLAN_SHARE: usize = 4_000;
const RESULT_SHARE: usize = 5_000;
const DIFF_SHARE: usize = 7_000;
const QUESTION_SHARE: usize = 2_000;
/// The most of an answer read from the CLI's output.
const OUTPUT_LIMIT: usize = 4 * 1024 * 1024;

/// What `/advisor` or `/committee` asked: the agents named (none: pick) and the person's question.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdvisorCommand {
    pub committee: bool,
    pub agents: Vec<String>,
    pub question: Option<String>,
}

/// An agent's name as typed: its provider id, or one of the names the app shows.
pub fn agent_named(word: &str) -> Option<&'static str> {
    match word.to_ascii_lowercase().as_str() {
        "claude" => Some("claude"),
        "codex" => Some("codex"),
        "gemini" | "antigravity" | "agy" => Some("gemini"),
        "pi" => Some("pi"),
        "opencode" => Some("opencode"),
        "hermes" => Some("hermes"),
        _ => None,
    }
}

/// `/advisor [agent] [question]` or `/committee [agent [agent]] [question]` as typed in the composer; None for any other text.
pub fn parse_command(text: &str) -> Option<AdvisorCommand> {
    let text = text.trim();
    let (committee, rest) = match text.strip_prefix("/advisor") {
        Some(rest) => (false, rest),
        None => (true, text.strip_prefix("/committee")?),
    };
    if !(rest.is_empty() || rest.starts_with(char::is_whitespace)) {
        return None;
    }
    let mut agents = Vec::new();
    let mut rest = rest.trim_start();
    let wanted = if committee { 2 } else { 1 };
    while agents.len() < wanted {
        let word = rest.split_whitespace().next().unwrap_or("");
        let Some(agent) = agent_named(word) else { break };
        if !agents.iter().any(|a| a == agent) {
            agents.push(agent.to_string());
        }
        rest = rest[word.len()..].trim_start();
    }
    let question = Some(rest.trim().to_string()).filter(|q| !q.is_empty());
    Some(AdvisorCommand { committee, agents, question })
}

/// Why no agent could be picked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PickRefusal {
    /// The agent named cannot give a read-only opinion.
    Unsupported(String),
    /// The agent named is the pane's own.
    Same,
    /// The agent named is not installed or not signed in.
    Unavailable(String),
    /// No other installed and signed-in agent.
    None,
}

/// The agents that answer: the ones named (each read-only capable, not the pane's own, installed and signed in: `ready`), else the
/// first `count` of [`ADVISORS`] that are not the pane's own and are ready.
pub fn pick_agents(own: &str, named: &[String], count: usize, ready: &dyn Fn(&str) -> bool) -> Result<Vec<String>, PickRefusal> {
    let mut picked = Vec::new();
    for agent in named {
        if !ADVISORS.contains(&agent.as_str()) {
            return Err(PickRefusal::Unsupported(agent.clone()));
        }
        if agent == own {
            return Err(PickRefusal::Same);
        }
        if !ready(agent) {
            return Err(PickRefusal::Unavailable(agent.clone()));
        }
        picked.push(agent.clone());
    }
    for agent in ADVISORS {
        if picked.len() >= count {
            break;
        }
        if agent != own && !picked.iter().any(|p| p == agent) && ready(agent) {
            picked.push(agent.to_string());
        }
    }
    if picked.is_empty() { Err(PickRefusal::None) } else { Ok(picked) }
}

fn cut(text: &str, limit: usize) -> String {
    let trimmed = text.trim();
    if trimmed.chars().count() <= limit {
        return trimmed.to_string();
    }
    format!("{}\n[… cut]", trimmed.chars().take(limit).collect::<String>())
}

/// What the advisor is asked. Every part is redacted by the host's patterns and cut to its share of [`PROMPT_LIMIT`].
pub fn prompt(own: &str, plan: Option<&str>, result: &str, diff: &str, question: Option<&str>, redact: &dyn Fn(&str) -> String) -> String {
    let mut out = format!(
        "[Mighty Code] You are asked for a second opinion on work another coding agent ({}) did in this repository. You may read files to check it, but change nothing: do not edit, write, run commands or install anything. Answer in the language of the work below. Be concrete and brief: what looks right, what looks wrong or risky, and what you would do differently.\n",
        mighty_run::providers::name(own)
    );
    if let Some(plan) = plan.filter(|p| !p.trim().is_empty()) {
        out.push_str("\n## Its plan\n");
        out.push_str(&redact(&cut(plan, PLAN_SHARE)));
        out.push('\n');
    }
    if !result.trim().is_empty() {
        out.push_str("\n## Its last answer\n");
        out.push_str(&redact(&cut(result, RESULT_SHARE)));
        out.push('\n');
    }
    if !diff.trim().is_empty() {
        out.push_str("\n## What its last request changed (summary)\n```diff\n");
        out.push_str(&cut(diff, DIFF_SHARE));
        out.push_str("\n```\n");
    }
    out.push_str("\n## The question\n");
    match question {
        Some(question) => out.push_str(&redact(&cut(question, QUESTION_SHARE))),
        None => out.push_str("Review the plan and the work. Is this the right approach? What would you change?"),
    }
    out.push('\n');
    if out.chars().count() > PROMPT_LIMIT {
        out = cut(&out, PROMPT_LIMIT - 20);
    }
    out
}

/// How an advisor runs: its arguments after the CLI's own prefix, and what goes to its standard input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdvisorLaunch {
    pub args: Vec<String>,
    pub stdin: Option<String>,
}

/// The one-off read-only run of `provider` for `prompt` (`model` when the person's last pane of that agent named one); None for an
/// agent that has no read-only one-shot mode.
pub fn launch(provider: &str, model: Option<&str>, prompt: &str) -> Option<AdvisorLaunch> {
    let strings = |items: &[&str]| items.iter().map(|s| s.to_string()).collect::<Vec<String>>();
    let model = model.filter(|m| *m != "default" && wire::model(m));
    let launch = match provider {
        "claude" => {
            let mut args = strings(&[
                "--print",
                "--output-format",
                "json",
                "--permission-mode",
                "plan",
                "--permission-prompts",
                "none",
                "--tools",
                "Read,Grep,Glob",
                "--disallowedTools",
                "Bash,Edit,Write,MultiEdit,NotebookEdit,WebFetch,WebSearch,Task",
                "--no-session-persistence",
                "--strict-mcp-config",
            ]);
            if let Some(model) = model {
                args.extend(["--model".to_string(), model.to_string()]);
            }
            AdvisorLaunch { args, stdin: Some(prompt.to_string()) }
        }
        "codex" => {
            let mut args = strings(&[
                "-c",
                "approval_policy=\"never\"",
                "-c",
                "sandbox_mode=\"read-only\"",
                "exec",
                "--json",
                "--skip-git-repo-check",
                "--sandbox",
                "read-only",
                "--ephemeral",
            ]);
            if let Some(model) = model {
                args.extend(["--model".to_string(), model.to_string()]);
            }
            args.push("-".into());
            AdvisorLaunch { args, stdin: Some(prompt.to_string()) }
        }
        "gemini" => {
            let mut args = vec![format!("--print={prompt}")];
            args.extend(strings(&["--output-format", "text", "--mode", "plan", "--sandbox", "--print-timeout", "0"]));
            if let Some(model) = model {
                args.extend(["--model".to_string(), model.to_string()]);
            }
            AdvisorLaunch { args, stdin: None }
        }
        "pi" => {
            let mut args = strings(&["--print", "--no-session", "--no-mcp", "--tools", "read,grep,find,ls"]);
            if let Some(model) = model {
                args.extend(["--model".to_string(), model.to_string()]);
            }
            args.push(prompt.to_string());
            AdvisorLaunch { args, stdin: None }
        }
        _ => return None,
    };
    read_only(provider, &launch.args).then_some(launch)
}

/// Whether `args` keep `provider`'s run from changing anything: its read-only mode named, nothing that lifts it, no tool that writes
/// or runs commands left on. Every advisor run is checked with it before it starts.
pub fn read_only(provider: &str, args: &[String]) -> bool {
    let has = |flag: &str| args.iter().any(|a| a == flag);
    let after = |flag: &str| args.iter().position(|a| a == flag).and_then(|at| args.get(at + 1)).map(String::as_str);
    let lifts = ["--dangerously-skip-permissions", "--dangerously-bypass-approvals-and-sandbox", "--yolo", "--auto", "--full-auto"];
    if args.iter().any(|a| lifts.contains(&a.as_str()) || a.starts_with("--allow-dangerously")) {
        return false;
    }
    match provider {
        "claude" => {
            let tools = after("--tools").unwrap_or("default");
            after("--permission-mode") == Some("plan")
                && tools != "default"
                && tools.split(',').all(|t| matches!(t.trim(), "Read" | "Grep" | "Glob" | ""))
                && after("--disallowedTools").is_some_and(|d| ["Bash", "Edit", "Write"].iter().all(|t| d.split(',').any(|x| x == *t)))
                && has("--no-session-persistence")
                && !has("--allowedTools")
        }
        "codex" => {
            after("--sandbox") == Some("read-only")
                && args.iter().any(|a| a == "approval_policy=\"never\"")
                && !args.iter().any(|a| a.starts_with("sandbox_mode=") && a != "sandbox_mode=\"read-only\"")
                && has("exec")
                && !has("app-server")
        }
        "gemini" => after("--mode") == Some("plan") && has("--sandbox"),
        "pi" => {
            after("--tools").is_some_and(|t| t.split(',').all(|x| matches!(x, "read" | "grep" | "find" | "ls")))
                && has("--no-session")
                && has("--no-mcp")
        }
        _ => false,
    }
}

/// An advisor's answer as its output says it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Answer {
    pub body: String,
    pub model: Option<String>,
    pub cost_usd: Option<f64>,
    pub tokens: Option<i64>,
}

fn sum_tokens(usage: &Value) -> Option<i64> {
    let fields = ["input_tokens", "output_tokens", "cache_read_input_tokens", "cache_creation_input_tokens", "cached_input_tokens"];
    let total: i64 = fields.iter().filter_map(|f| usage.get(*f).and_then(Value::as_i64)).filter(|n| *n >= 0).sum();
    (total > 0).then_some(total)
}

/// What `provider` printed, read: Claude's JSON result (its text, cost, tokens and the model that answered), Codex's JSONL events (its
/// last message and the turn's tokens); Antigravity and pi print the answer as text.
pub fn parse_output(provider: &str, stdout: &str) -> Answer {
    match provider {
        "claude" => {
            let Ok(root) = serde_json::from_str::<Value>(stdout.trim()) else {
                return Answer { body: stdout.trim().to_string(), ..Answer::default() };
            };
            let root = match &root {
                Value::Array(items) => {
                    items.iter().rev().find(|i| i.get("type").and_then(Value::as_str) == Some("result")).cloned().unwrap_or(Value::Null)
                }
                _ => root,
            };
            let model = root.get("modelUsage").and_then(Value::as_object).and_then(|m| {
                m.iter().max_by_key(|(_, v)| v.get("outputTokens").and_then(Value::as_i64).unwrap_or(0)).map(|(k, _)| k.clone())
            });
            Answer {
                body: root.get("result").and_then(Value::as_str).unwrap_or("").trim().to_string(),
                model,
                cost_usd: root.get("total_cost_usd").and_then(Value::as_f64).filter(|c| c.is_finite() && *c >= 0.0),
                tokens: root.get("usage").and_then(sum_tokens),
            }
        }
        "codex" => {
            let mut answer = Answer::default();
            for line in stdout.lines() {
                let Ok(event) = serde_json::from_str::<Value>(line.trim()) else { continue };
                match event.get("type").and_then(Value::as_str) {
                    Some("item.completed") => {
                        if let Some(item) = event.get("item").filter(|i| i.get("type").and_then(Value::as_str) == Some("agent_message")) {
                            answer.body = item.get("text").and_then(Value::as_str).unwrap_or("").trim().to_string();
                        }
                    }
                    Some("turn.completed") => {
                        if let Some(tokens) = event.get("usage").and_then(sum_tokens) {
                            answer.tokens = Some(answer.tokens.unwrap_or(0) + tokens);
                        }
                    }
                    _ => {}
                }
            }
            answer
        }
        _ => Answer { body: stdout.trim().to_string(), ..Answer::default() },
    }
}

// ── the host ───────────────────────────────────────────────────────────────────────────────────────────────────────────────

fn update_card(host: &Arc<Host>, pane_id: &str, card: OpinionCard) {
    let _ = host.view().update(|state| {
        if let Some(pane) = state.sessions_mut().iter_mut().find(|p| p.id() == pane_id) {
            let mut next = RunSession::clone(pane);
            let cards = next.opinions.get_or_insert_with(Vec::new);
            match cards.iter_mut().find(|c| c.id == card.id) {
                Some(held) => *held = card,
                None => cards.push(card),
            }
            let skip = cards.len().saturating_sub(OPINION_CARDS_KEPT);
            cards.drain(..skip);
            *pane = Arc::new(next);
        }
    });
    host.schedule();
}

/// The opinion card `card_id` taken off pane `pane_id` (the card's 닫기).
pub fn dismiss(host: &Arc<Host>, pane_id: &str, card_id: &str) {
    let _ = host.view().update(|state| {
        if let Some(pane) = state.sessions_mut().iter_mut().find(|p| p.id() == pane_id) {
            let mut next = RunSession::clone(pane);
            if let Some(cards) = next.opinions.as_mut() {
                cards.retain(|c| c.id != card_id || c.state == OpinionCard::RUNNING);
            }
            if next.opinions.as_ref().is_some_and(Vec::is_empty) {
                next.opinions = None;
            }
            *pane = Arc::new(next);
        }
    });
    host.schedule();
}

/// A line in the pane's transcript instead of an answer (why nothing was asked).
fn note(host: &Arc<Host>, pane_id: &str, text: String) {
    let mut view = host.view();
    let _ = crate::composer::slash_note(&mut view, pane_id, &text);
    drop(view);
    host.schedule();
}

/// The summary of what the pane's last request changed: from the request's own snapshots (its first and last checkpoint) when it has
/// them, else what is not committed in its folder; the stat, then the diff, secret-looking lines redacted, cut to its share.
async fn diff_summary(pane: &RunSession, folder: &str) -> String {
    let Some(places) = crate::pane_worktree::places() else { return String::new() };
    let run = pane.graph_runs().iter().rev().find(|r| r.notice_run != Some(true));
    let checkpoints = run.map(|r| r.checkpoints()).unwrap_or_default();
    let range: Vec<String> = match (checkpoints.first(), checkpoints.last()) {
        (Some(first), Some(last)) if first.tree != last.tree && wire::identifier(&first.tree) && wire::identifier(&last.tree) => {
            vec![first.tree.clone(), last.tree.clone()]
        }
        _ => vec!["HEAD".to_string()],
    };
    let mut stat_args = vec!["diff", "--stat", "--no-color", "--no-ext-diff"];
    let mut diff_args = vec!["diff", "-U2", "--no-color", "--no-ext-diff", "--no-textconv"];
    for part in &range {
        stat_args.push(part);
        diff_args.push(part);
    }
    let read = |args: Vec<&str>| {
        let git = places.git.clone();
        let args: Vec<String> = args.into_iter().map(str::to_string).collect();
        let folder = folder.to_string();
        async move {
            let borrowed: Vec<&str> = args.iter().map(String::as_str).collect();
            mighty_delegation::git::run(&git, &borrowed, &folder, 30.0)
                .await
                .filter(|r| r.exit_code == 0)
                .map(|r| r.stdout)
                .unwrap_or_default()
        }
    };
    let stat = read(stat_args).await;
    let diff = read(diff_args).await;
    let text = format!("{}\n{}", stat.trim_end(), crate::git_workbench::redact_secret_lines(&diff));
    text.chars().take(DIFF_SHARE).collect()
}

/// Whether `provider` is installed and signed in, as the accounts box last read it (read now when it has not been).
async fn ready(host: &Arc<Host>, provider: &str) -> bool {
    let Some(runs) = host.runs().cloned() else { return false };
    if runs.catalog.find(provider).await.is_none() {
        return false;
    }
    crate::accounts::signed_in(host, provider).await
}

/// `/advisor` or `/committee` typed in pane `pane_id`: the agents picked, a card for each put on the pane at once, and each asked on a
/// task of its own.
pub async fn start(host: &Arc<Host>, pane_id: &str, command: AdvisorCommand) {
    let locale = host.locale;
    crate::bodies::ensure(host, &[pane_id]);
    let (pane, workspace) = {
        let view = host.view();
        let Some(pane) = view.snapshot.session(pane_id).cloned() else { return };
        let workspace = view.snapshot.workspace(pane.workspace_id()).cloned();
        (pane, workspace)
    };
    let Some(workspace) = workspace.filter(|w| w.remote.is_none() && !crate::mirror::is_mirror_workspace(&host.view().snapshot, w.id()))
    else {
        return note(host, pane_id, locale.get("advisor.note.remote"));
    };
    let Some(point) = mighty_run::fork::fork_point(&pane, None).filter(|p| !p.result.trim().is_empty() || p.plan.is_some()) else {
        return note(host, pane_id, locale.get("advisor.note.nothing"));
    };
    let count = if command.committee { 2 } else { 1 };
    // Only as many agents are asked whether they are signed in as the pick needs: the ones named, then the first free ones in order.
    let mut readiness = BTreeMap::new();
    for agent in command.agents.iter().filter(|a| ADVISORS.contains(&a.as_str()) && a.as_str() != pane.provider()) {
        if !readiness.contains_key(agent) {
            let is_ready = ready(host, agent).await;
            readiness.insert(agent.clone(), is_ready);
        }
    }
    let mut found = readiness.values().filter(|r| **r).count();
    for agent in ADVISORS {
        if found >= count {
            break;
        }
        if agent == pane.provider() || readiness.contains_key(agent) {
            continue;
        }
        let is_ready = ready(host, agent).await;
        found += usize::from(is_ready);
        readiness.insert(agent.to_string(), is_ready);
    }
    let agents = match pick_agents(pane.provider(), &command.agents, count, &|a| readiness.get(a).copied().unwrap_or(false)) {
        Ok(agents) => agents,
        Err(refusal) => {
            let names = ADVISORS.iter().map(|a| mighty_run::providers::name(a)).collect::<Vec<_>>().join(", ");
            let text = match refusal {
                PickRefusal::Unsupported(agent) => {
                    locale.get_with("advisor.note.unknownAgent", &[("agent", mighty_run::providers::name(&agent)), ("agents", &names)])
                }
                PickRefusal::Same => locale.get("advisor.note.sameAgent"),
                PickRefusal::Unavailable(agent) => {
                    locale.get_with("advisor.note.unavailable", &[("agent", mighty_run::providers::name(&agent))])
                }
                PickRefusal::None => locale.get("advisor.note.noAgent"),
            };
            return note(host, pane_id, text);
        }
    };
    let folder = pane.working_folder.clone().unwrap_or_else(|| workspace.path().to_string());
    let diff = diff_summary(&pane, &folder).await;
    let asked = prompt(
        pane.provider(),
        point.plan.as_deref(),
        &point.result,
        &diff,
        command.question.as_deref(),
        &crate::git_workbench::redact_secret_text,
    );
    let group = (agents.len() > 1).then(wire::new_id);
    for agent in agents {
        let card = OpinionCard {
            id: wire::new_id(),
            provider: agent.clone(),
            model: None,
            cost_usd: None,
            tokens: None,
            question: command.question.clone(),
            body: String::new(),
            state: OpinionCard::RUNNING.into(),
            created_at: Some(time::now_o()),
            group: group.clone(),
        };
        update_card(host, pane_id, card.clone());
        // The model the person's own panes of that agent use, when one names one.
        let model = host
            .view()
            .snapshot
            .sessions()
            .iter()
            .rev()
            .find(|s| s.kind() == "claude" && s.provider() == agent && s.model() != "default")
            .map(|s| s.model().to_string());
        let (host, pane_id, folder, asked, workspace_id) =
            (host.clone(), pane_id.to_string(), folder.clone(), asked.clone(), workspace.id().to_string());
        tauri::async_runtime::spawn(async move {
            let outcome = ask(&host, &agent, model.as_deref(), &asked, &folder).await;
            let finished = match outcome {
                Ok(answer) if !answer.body.is_empty() => {
                    let shown_model = answer.model.clone().or(model.clone());
                    crate::usage_report::record_advisor(
                        &host,
                        &workspace_id,
                        &agent,
                        shown_model.as_deref().unwrap_or("default"),
                        answer.cost_usd,
                        answer.tokens,
                    );
                    OpinionCard {
                        model: shown_model,
                        cost_usd: answer.cost_usd,
                        tokens: answer.tokens,
                        body: answer.body.chars().take(OPINION_BODY_LIMIT).collect(),
                        state: OpinionCard::DONE.into(),
                        ..card
                    }
                }
                Ok(_) => OpinionCard { body: host.locale.get("advisor.error.empty"), state: OpinionCard::FAILED.into(), ..card },
                Err(why) => OpinionCard { body: why, state: OpinionCard::FAILED.into(), ..card },
            };
            if host.view().snapshot.session(&pane_id).is_some() {
                update_card(&host, &pane_id, finished);
            }
        });
    }
}

/// One advisor run: the CLI found, started in the pane's folder (read only) with the checked arguments, the prompt written, its output
/// read to the end or [`TIME_LIMIT`].
async fn ask(host: &Arc<Host>, provider: &str, model: Option<&str>, prompt: &str, folder: &str) -> Result<Answer, String> {
    let locale = host.locale;
    let name = mighty_run::providers::name(provider);
    let failed = |reason: &str| locale.get_with("advisor.error.failed", &[("agent", name), ("reason", reason)]);
    let runs = host.runs().cloned().ok_or_else(|| failed("no run engine"))?;
    let command = runs.catalog.find(provider).await.ok_or_else(|| locale.get_with("advisor.note.unavailable", &[("agent", name)]))?;
    let Some(launch) = launch(provider, model, prompt) else { return Err(failed("no read-only mode")) };
    let environment = if provider == "claude" {
        mighty_run::providers::claude_headless_environment(runs.catalog.environment().as_ref())
    } else {
        runs.catalog.quiet_environment()
    };
    let mut args = command.prefix.clone();
    args.extend(launch.args.iter().cloned());
    let request = host
        .processes
        .cli_request(Path::new(&command.binary), &args, Path::new(folder), &environment)
        .map_err(|e| failed(&e.to_string()))?;
    let mut child = host.processes.spawn(request).await.map_err(|e| failed(&e.to_string()))?;
    let control = child.control();
    let errors = child.stderr.take().map(|mut stderr| {
        tokio::spawn(async move {
            let mut kept = Vec::new();
            let mut buffer = [0u8; 4096];
            while let Ok(read) = stderr.read(&mut buffer).await {
                if read == 0 {
                    break;
                }
                kept.extend_from_slice(&buffer[..read]);
                if kept.len() > 16_384 {
                    kept.drain(..kept.len() - 16_384);
                }
            }
            String::from_utf8_lossy(&kept).into_owned()
        })
    });
    let (stdin, stdout, input) = (child.stdin.take(), child.stdout.take(), launch.stdin.clone());
    let work = async move {
        if let Some(mut stdin) = stdin {
            if let Some(text) = &input {
                let _ = stdin.write_all(text.as_bytes()).await;
                let _ = stdin.flush().await;
            }
            drop(stdin);
        }
        let mut output = Vec::new();
        if let Some(mut stdout) = stdout {
            let mut buffer = [0u8; 8192];
            while let Ok(read) = stdout.read(&mut buffer).await {
                if read == 0 || output.len() > OUTPUT_LIMIT {
                    break;
                }
                output.extend_from_slice(&buffer[..read]);
            }
        }
        String::from_utf8_lossy(&output).into_owned()
    };
    let outcome = tokio::time::timeout(TIME_LIMIT, work).await;
    control.kill();
    let _ = control.close().await;
    let error_text = match errors {
        Some(task) => tokio::time::timeout(Duration::from_secs(1), task).await.ok().and_then(Result::ok).unwrap_or_default(),
        None => String::new(),
    };
    let Ok(stdout) = outcome else { return Err(locale.get("advisor.error.timeout")) };
    let answer = parse_output(provider, &stdout);
    if answer.body.is_empty() {
        let tail: String = crate::git_workbench::redact_secret_text(error_text.trim())
            .chars()
            .rev()
            .take(400)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        if !tail.is_empty() {
            return Err(failed(&tail));
        }
    }
    Ok(answer)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_command_names_agents_and_a_question() {
        assert_eq!(parse_command("/advisor"), Some(AdvisorCommand { committee: false, agents: vec![], question: None }));
        assert_eq!(
            parse_command("/advisor codex is the plan sound?"),
            Some(AdvisorCommand { committee: false, agents: vec!["codex".into()], question: Some("is the plan sound?".into()) })
        );
        assert_eq!(
            parse_command("/advisor why not a cache?"),
            Some(AdvisorCommand { committee: false, agents: vec![], question: Some("why not a cache?".into()) })
        );
        assert_eq!(
            parse_command("/committee antigravity claude 원인이 뭘까"),
            Some(AdvisorCommand {
                committee: true, agents: vec!["gemini".into(), "claude".into()], question: Some("원인이 뭘까".into())
            })
        );
        assert_eq!(parse_command("/advisors"), None);
        assert_eq!(parse_command("please /advisor"), None);
    }

    #[test]
    fn the_agent_is_another_installed_and_signed_in_one() {
        let all = |_: &str| true;
        assert_eq!(pick_agents("claude", &[], 1, &all), Ok(vec!["codex".to_string()]), "the first that is not the pane's own");
        assert_eq!(pick_agents("codex", &[], 1, &all), Ok(vec!["claude".to_string()]));
        assert_eq!(pick_agents("claude", &[], 2, &all), Ok(vec!["codex".to_string(), "gemini".to_string()]), "a committee of two");
        let only_pi = |a: &str| a == "pi";
        assert_eq!(pick_agents("claude", &[], 1, &only_pi), Ok(vec!["pi".to_string()]), "skips the ones not ready");
        assert_eq!(pick_agents("claude", &["claude".into()], 1, &all), Err(PickRefusal::Same));
        assert_eq!(pick_agents("claude", &["hermes".into()], 1, &all), Err(PickRefusal::Unsupported("hermes".into())));
        assert_eq!(pick_agents("claude", &["codex".into()], 1, &only_pi), Err(PickRefusal::Unavailable("codex".into())));
        assert_eq!(pick_agents("claude", &[], 1, &|_| false), Err(PickRefusal::None));
    }

    #[test]
    fn every_advisor_runs_read_only() {
        for provider in ADVISORS {
            let made = launch(provider, Some("some-model"), "question").unwrap_or_else(|| panic!("{provider} has a read-only launch"));
            assert!(read_only(provider, &made.args), "{provider}: {:?}", made.args);
        }
        let claude = launch("claude", None, "q").unwrap().args;
        assert!(claude.windows(2).any(|w| w == ["--permission-mode", "plan"]));
        assert!(!claude.iter().any(|a| a.contains("Bash") && !a.contains("Edit")), "Bash only ever in the denied list");
        let codex = launch("codex", None, "q").unwrap().args;
        assert!(codex.windows(2).any(|w| w == ["--sandbox", "read-only"]) && codex.contains(&"--ephemeral".to_string()));
        let pi = launch("pi", None, "q").unwrap().args;
        assert!(pi.windows(2).any(|w| w == ["--tools", "read,grep,find,ls"]));
        assert!(launch("opencode", None, "q").is_none() && launch("hermes", None, "q").is_none(), "no read-only one-shot there");
    }

    #[test]
    fn a_write_or_a_lifted_sandbox_is_refused() {
        let with = |provider: &str, change: &dyn Fn(&mut Vec<String>)| {
            let mut args = launch(provider, None, "q").unwrap().args;
            change(&mut args);
            read_only(provider, &args)
        };
        assert!(!with("claude", &|a| a.push("--dangerously-skip-permissions".into())));
        assert!(!with("claude", &|a| {
            let at = a.iter().position(|x| x == "--tools").unwrap();
            a[at + 1] = "Read,Edit,Bash".into();
        }));
        assert!(!with("claude", &|a| {
            let at = a.iter().position(|x| x == "plan").unwrap();
            a[at] = "acceptEdits".into();
        }));
        assert!(!with("claude", &|a| a.extend(["--allowedTools".into(), "Bash".into()])));
        assert!(!with("codex", &|a| {
            let at = a.iter().position(|x| x == "--sandbox").unwrap();
            a[at + 1] = "workspace-write".into();
        }));
        assert!(!with("codex", &|a| a.push("--dangerously-bypass-approvals-and-sandbox".into())));
        assert!(!with("gemini", &|a| a.push("--dangerously-skip-permissions".into())));
        assert!(!with("pi", &|a| {
            let at = a.iter().position(|x| x == "--tools").unwrap();
            a[at + 1] = "read,bash,edit,write".into();
        }));
        assert!(!read_only("hermes", &["-z".into(), "q".into()]), "an agent with no read-only mode is never run");
    }

    #[test]
    fn the_prompt_is_capped_and_redacted() {
        let redact = |t: &str| crate::git_workbench::redact_secret_text(t);
        let result = format!("ghp_abcdefghijklmnopqrstuvwxyz0123456789AB was used\n{}", "word ".repeat(10_000));
        let text = prompt("claude", Some("1. step"), &result, &"+line\n".repeat(5_000), Some("is it right?"), &redact);
        assert!(text.chars().count() <= PROMPT_LIMIT, "{}", text.chars().count());
        assert!(!text.contains("ghp_abcdefghijklmnopqrstuvwxyz0123456789AB"), "secrets never leave");
        assert!(text.contains("1. step") && text.contains("Claude"));
        assert!(text.contains("change nothing"));
    }

    #[test]
    fn answers_are_read_from_each_output() {
        let claude = r#"{"type":"result","subtype":"success","result":"Looks fine.","total_cost_usd":0.0123,"usage":{"input_tokens":100,"output_tokens":20},"modelUsage":{"claude-opus-5-5":{"outputTokens":20}}}"#;
        assert_eq!(
            parse_output("claude", claude),
            Answer { body: "Looks fine.".into(), model: Some("claude-opus-5-5".into()), cost_usd: Some(0.0123), tokens: Some(120) }
        );
        let codex = "{\"type\":\"thread.started\",\"thread_id\":\"t\"}\n{\"type\":\"item.completed\",\"item\":{\"id\":\"i\",\"type\":\"agent_message\",\"text\":\"Use a lock.\"}}\n{\"type\":\"turn.completed\",\"usage\":{\"input_tokens\":50,\"cached_input_tokens\":10,\"output_tokens\":5}}\n";
        let answer = parse_output("codex", codex);
        assert_eq!((answer.body.as_str(), answer.tokens), ("Use a lock.", Some(65)));
        assert_eq!(parse_output("pi", "  plain text\n").body, "plain text");
    }

    /// An opinion card saved with its pane comes back as it was; one still running when the app closed does not.
    #[test]
    fn opinion_cards_save_and_restore() {
        use mighty_core::json::Wire;
        let done = OpinionCard {
            id: "c1".into(),
            provider: "codex".into(),
            model: Some("gpt-6-astra".into()),
            cost_usd: Some(0.02),
            tokens: Some(1200),
            question: Some("is it right?".into()),
            body: "Use a lock.".into(),
            state: OpinionCard::DONE.into(),
            created_at: Some("2026-11-23T00:00:00Z".into()),
            group: None,
        };
        let running = OpinionCard { id: "c2".into(), state: OpinionCard::RUNNING.into(), body: String::new(), ..done.clone() };
        let pane = RunSession { id: Some("p".into()), opinions: Some(vec![done.clone(), running.clone()]), ..RunSession::default() };
        let read = RunSession::read(&serde_json::from_str(&pane.to_json()).unwrap()).unwrap();
        assert_eq!(read.opinions.as_deref(), Some(&[done.clone(), running.clone()][..]));
        assert_eq!(
            mighty_core::normalize::opinions(read.opinions.as_ref(), true),
            Some(vec![done.clone()]),
            "a restore drops a running card"
        );
        assert_eq!(mighty_core::normalize::opinions(read.opinions.as_ref(), false).map(|c| c.len()), Some(2));
        let many: Vec<OpinionCard> = (0..10).map(|i| OpinionCard { id: format!("c{i}"), ..done.clone() }).collect();
        let kept = mighty_core::normalize::opinions(Some(&many), false).unwrap();
        assert_eq!(kept.len(), OPINION_CARDS_KEPT);
        assert_eq!(kept.last().unwrap().id, "c9", "the newest stay");
    }
}
