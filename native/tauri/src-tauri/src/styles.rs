//! Styles in the host (C# `WinUI\MainWindow.Styles.cs`, `MainWindow.StyleApproval.cs`, `MainWindow.WebShell.Guided.cs`,
//! `MainWindow.Terminal.cs:20` `RunTerminalCommandAsync`, the style parts of `MainWindow.MobileRemote.cs`; plan §1.7, W9-D). Owner: W9-D.
//!
//! mighty-styles' registry, trust store and projection for each pane: the composer's `style` row and `guided` panel, the style menu
//! rows, `guidedAction{paneId, what, id}`, Enter rewritten to the style's slash command, the approval, remove and install sheets
//! (D11–D13), the install through a command typed into a new shell pane where the host may change the machine, the plan-mode launch,
//! the Mighty views' `StyleSource`, the per-request auto-allow, the change watcher (off in a smoke) and the phone's style parts.
//! Writes (`approvals.json`, a registered manifest copied to `<data>\styles`, a removal) only where `Host::writes` allows.
//!
//! What C# keeps per `PaneView` (`styleRegistry`, `activeStyle`, `styleReading`, `stylePrerequisites`, `styleGroup`,
//! `styleStartingNew`, `styleLoading`, `styleAgain`, `loadedStyleKey`) is [`PaneStyle`], one per pane, behind its own lock: taken
//! after the view's when both are held, never while the view is asked for.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex, MutexGuard};
use std::time::Duration;

use mighty_composer::composer::StyleRow;
use mighty_composer::menus::{MenuAction, MenuRow};
use mighty_core::locale::Locale;
use mighty_core::model::{AppSnapshot, RunSession};
use mighty_core::permission::ToolPermissionRequest;
use mighty_run::model::StartRunRequest;
use mighty_styles::evaluator::StyleStateReading;
use mighty_styles::manifest::StylePhase;
use mighty_styles::permissions::StyleAllowing;
use mighty_styles::watcher::StyleChangeWatcher;
use mighty_styles::{
    PlainAccess, RegisteredStyle, StylePrerequisiteResult, StyleRegistry, StyleTrustStore, TrustAccess, decoder, permissions,
    prerequisites, presentation, state,
};
use serde_json::{Value, json};

use crate::host::{Host, Registry, View};

// ── access ──────────────────────────────────────────────────────────────────────────────────────────────────────────────────

/// The trust store's folder checks on Windows (C# `Private`/`Restrict`): a real folder, never a junction or link, put back to this
/// user's protected list on every write (the setup screen's `prepare_folder`); the file inherits the folder's list. Reading a foreign
/// access list without writing one has no port, so `private` checks the links and the folder's kind.
#[cfg(windows)]
struct HostAccess(Arc<dyn mighty_platform::SetupFiles>);

#[cfg(windows)]
impl TrustAccess for HostAccess {
    fn private(&self, path: &Path, directory: bool) -> bool {
        PlainAccess.private(path, directory) && (!directory || self.0.is_plain_folder(path))
    }
    fn restrict(&self, path: &Path, directory: bool) -> std::io::Result<()> {
        if directory { self.0.prepare_folder(path).map_err(|e| std::io::Error::other(e.to_string())) } else { Ok(()) }
    }
}

/// The trust store's access checks for this OS.
pub fn access() -> Arc<dyn TrustAccess> {
    #[cfg(windows)]
    {
        Arc::new(HostAccess(crate::platform::setup_files()))
    }
    #[cfg(not(windows))]
    {
        Arc::new(PlainAccess)
    }
}

fn trust(host: &Host) -> StyleTrustStore {
    StyleTrustStore::new(&host.data_dir.join("style-trust"), access(), host.locale)
}

fn load_registry(host: &Host, workspace: &Path) -> StyleRegistry {
    StyleRegistry::load(&host.data_dir, workspace, access(), host.locale)
}

/// The prerequisites' home: the user's, or the smoke's own profile (C# `owner.options.SmokeTest ? StateDirectory : null`).
fn home(host: &Host) -> PathBuf {
    if host.smoke.is_some() { host.data_dir.clone() } else { PathBuf::from(crate::accounts::home()) }
}

fn approval_required(host: &Host) -> String {
    host.locale.get("guidedPanel.approvalRequired")
}

// ── the pane's style ────────────────────────────────────────────────────────────────────────────────────────────────────────

#[derive(Default)]
struct PaneStyle {
    registry: Option<Arc<StyleRegistry>>,
    active: Option<RegisteredStyle>,
    reading: Option<StyleStateReading>,
    prerequisites: Option<StylePrerequisiteResult>,
    group: Option<String>,
    starting_new: bool,
    loading: bool,
    again: bool,
    loaded_key: Option<String>,
    workspace: Option<PathBuf>,
}

static PANES: LazyLock<Mutex<HashMap<String, PaneStyle>>> = LazyLock::new(|| Mutex::new(HashMap::new()));
static ALLOWING: LazyLock<StyleAllowing> = LazyLock::new(StyleAllowing::default);

fn panes() -> MutexGuard<'static, HashMap<String, PaneStyle>> {
    PANES.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// C# `LocalPath(workspace)`: the pane's workspace folder on this computer, None for a remote one.
pub fn local_workspace(snapshot: &AppSnapshot, pane: &RunSession) -> Option<PathBuf> {
    let workspace = snapshot.workspace(pane.workspace_id())?;
    (workspace.remote.is_none() && !workspace.path().is_empty()).then(|| PathBuf::from(workspace.path()))
}

/// The style rows show: a local Claude pane in the Mighty view (C# `RefreshStyles`'s `visible`).
pub fn available(snapshot: &AppSnapshot, pane: &RunSession) -> bool {
    mighty_remote::phone::mighty::guided_styles_available(pane.kind(), pane.provider(), pane.agent_view_mode.as_deref().unwrap_or(""))
        && local_workspace(snapshot, pane).is_some()
}

/// `loadedStyleKey`: what makes a pane read its style again.
fn key(pane: &RunSession) -> String {
    format!(
        "{}|{}|{}|{}|{}",
        pane.mighty_style.as_deref().unwrap_or(""),
        pane.mighty_style_hash.as_deref().unwrap_or(""),
        pane.mighty_style_since.as_deref().unwrap_or(""),
        pane.logs.as_ref().map_or(0, Vec::len),
        pane.status()
    )
}

/// The prompts a pane's phase is read from (C# `GuidedPhase`): its request blocks' inputs, else its user entries.
fn prompts(pane: &RunSession) -> Vec<String> {
    match pane.graph_runs.as_deref() {
        Some(runs) if !runs.is_empty() => {
            mighty_graph::support::request_runs(runs).iter().map(|r| r.input.clone().unwrap_or_default()).collect()
        }
        _ => mighty_styles::projection::session_prompts(pane),
    }
}

/// `LiveStyleReading(style)`.
fn live(entry: &PaneStyle, style: &RegisteredStyle, pane: &RunSession, pending_plan: bool, locale: &Locale) -> StyleStateReading {
    let without = mighty_graph::support::without_notice_runs(pane);
    state::live(entry.reading.as_ref(), &style.manifest, &without, pending_plan, locale)
}

/// `GuidedPhase(style)`.
fn guided_phase<'a>(
    entry: &PaneStyle,
    style: &'a RegisteredStyle,
    pane: &RunSession,
    pending_plan: bool,
    locale: &Locale,
) -> Option<&'a StylePhase> {
    let reading = live(entry, style, pane, pending_plan, locale);
    style.evaluator().current_phase(&prompts(pane), entry.reading.as_ref().map(|r| &r.files), reading.plan_stage.as_deref())
}

fn has_requests(pane: &RunSession) -> bool {
    pane.logs.as_deref().unwrap_or(&[]).iter().any(|l| l.kind.as_deref() == Some("user")) || !pane.graph_runs().is_empty()
}

/// The host as the tasks that outlive the call share it.
fn shared(host: &Host) -> Option<Arc<Host>> {
    use tauri::Manager;
    host.app.try_state::<Arc<Host>>().map(|h| h.inner().clone())
}

fn mark(host: &Arc<Host>, id: &str) {
    {
        let mut view = host.view();
        if let Some(ui) = view.panes.get_mut(id) {
            ui.dirty = true;
        }
        view.mighty.dirty(id);
    }
    crate::phone::changed(host);
}

/// C# `RefreshStyles`: a pane whose key moved reads its styles again; one outside the style rows forgets its style. Called while the
/// view is drawn: the read starts on its own task.
fn ensure(host: &Host, snapshot: &AppSnapshot, pane: &RunSession) {
    let id = pane.id().to_string();
    if !available(snapshot, pane) {
        if let Some(entry) = panes().get_mut(&id) {
            entry.active = None;
        }
        return;
    }
    let wanted = key(pane);
    let stale = panes().get(&id).is_none_or(|e| e.loaded_key.as_deref() != Some(&wanted) && !e.loading);
    if stale && let Some(host) = shared(host) {
        tauri::async_runtime::spawn(async move { reload(&host, &id, false) });
    }
}

/// C# `LoadStyles(quiet)`: the registry, the chosen style, its reading and prerequisites, off the dispatcher; kept when the pane still
/// points at the same style.
pub fn reload(host: &Arc<Host>, id: &str, quiet: bool) {
    let (pane, workspace) = {
        let view = host.view();
        let Some(pane) = view.snapshot.session(id).cloned() else { return };
        if !available(&view.snapshot, &pane) {
            return;
        }
        let workspace = local_workspace(&view.snapshot, &pane);
        (pane, workspace)
    };
    let Some(workspace) = workspace else { return };
    {
        let mut all = panes();
        let entry = all.entry(id.to_string()).or_default();
        if entry.loading {
            entry.again = true;
            return;
        }
        entry.loading = true;
    }
    let (host, id) = (host.clone(), id.to_string());
    tauri::async_runtime::spawn(async move {
        let wanted = key(&pane);
        let (reader, folder, chosen_pane) = (host.clone(), workspace.clone(), pane.clone());
        let data = tauri::async_runtime::spawn_blocking(move || {
            let registry = load_registry(&reader, &folder);
            let chosen = registry.runnable(chosen_pane.mighty_style.as_deref(), chosen_pane.mighty_style_hash.as_deref()).cloned();
            let since = mighty_core::time::parse(chosen_pane.mighty_style_since.as_deref());
            let without = mighty_graph::support::without_notice_runs(&chosen_pane).into_owned();
            let reading = chosen.as_ref().map(|c| state::read(&c.manifest, &folder, since, Some(&without), &reader.locale));
            let prerequisites = chosen.as_ref().map(|c| prerequisites::read(&c.manifest, &folder, &home(&reader)));
            (registry, chosen, reading, prerequisites)
        })
        .await;
        let same = host.view().snapshot.session(&id).is_some_and(|now| {
            now.mighty_style == pane.mighty_style
                && now.mighty_style_hash == pane.mighty_style_hash
                && now.mighty_style_since == pane.mighty_style_since
        });
        let again = {
            let mut all = panes();
            let entry = all.entry(id.clone()).or_default();
            entry.loading = false;
            match data {
                Ok((registry, chosen, reading, prerequisites)) if same => {
                    if entry.group.is_none() {
                        entry.group = chosen.as_ref().and_then(|c| c.evaluator().initial_group(&BTreeMap::new()).map(|g| g.id.clone()));
                    }
                    entry.registry = Some(Arc::new(registry));
                    entry.active = chosen;
                    entry.reading = reading;
                    entry.prerequisites = prerequisites;
                    entry.loaded_key = Some(wanted);
                    entry.workspace = Some(workspace);
                }
                _ => {
                    if quiet {
                        entry.loaded_key = Some(wanted);
                    }
                }
            }
            std::mem::take(&mut entry.again)
        };
        let launches = panes().get(&id).and_then(|e| e.active.as_ref().map(|s| s.evaluator().launch_permission_mode() == Some("plan")));
        if let Some(ui) = host.view().panes.get_mut(&id) {
            ui.view.launches_in_plan_mode = launches.unwrap_or(false);
        }
        mark(&host, &id);
        watch(&host);
        if again {
            reload(&host, &id, quiet);
        }
    });
}

// ── the composer ────────────────────────────────────────────────────────────────────────────────────────────────────────────

/// The style chip (C# `stylePicker`, `styleHint`): the chosen style's name and source badge, what it is for, off while the pane runs.
pub fn row(host: &Host, snapshot: &AppSnapshot, pane: &RunSession, starting: bool) -> Option<StyleRow> {
    ensure(host, snapshot, pane);
    if !available(snapshot, pane) {
        return None;
    }
    let l = host.locale;
    let all = panes();
    let active = all.get(pane.id()).and_then(|e| e.active.as_ref());
    Some(StyleRow {
        label: active.map_or_else(|| "CLI".to_string(), |s| s.manifest.name().to_string()),
        badge: active.filter(|s| s.source != "bundled").map(|s| presentation::source(&l, &s.source)),
        hint: active.map_or_else(|| l.get("composer.style.free"), |s| s.manifest.subtitle().to_string()),
        enabled: pane.status() != "running" && !starting,
    })
}

/// A manifest's tint as the page's ink (C# `StyleTint`).
fn tint(name: Option<&str>) -> &'static str {
    match name {
        Some("purple") => "agent-text",
        Some("teal") => "task-text",
        Some("indigo") => "question-text",
        Some("mint") => "compact-text",
        Some("orange") => "steer-text",
        Some("green") => "done-text",
        Some("red") => "err-text",
        Some("secondary") => "ink2",
        _ => "accent",
    }
}

/// What the composer holds that the guide panel reads.
pub struct Draft<'a> {
    pub text: &'a str,
    pub attachments: bool,
    pub starting: bool,
    pub requests: &'a [ToolPermissionRequest],
}

/// C# `RenderGuidedStyle` as the page's `GuidedPanel`: the approval strip for a style not said yes to, else the phase bar, the Enter
/// chip, what the style still needs, the state widgets, the group map, the chips (or the spinner), the reset chip and the guidance.
pub fn guided(host: &Host, snapshot: &AppSnapshot, pane: &RunSession, draft: &Draft<'_>) -> Option<Value> {
    if !available(snapshot, pane) {
        return None;
    }
    let l = host.locale;
    let all = panes();
    let entry = all.get(pane.id());
    let Some(style) = entry.and_then(|e| e.active.as_ref()) else {
        let id = pane.mighty_style.as_deref()?;
        let registry = entry.and_then(|e| e.registry.as_ref());
        let pending = registry.and_then(|r| r.styles.iter().find(|s| s.id() == id));
        return Some(json!({ "approval": {
            "name": pending.map_or(id, |s| s.manifest.name()),
            "source": pending.filter(|s| s.source != "bundled").map(|s| presentation::source(&l, &s.source)),
            "canView": pending.is_some_and(|s| s.approval != "revoked") && !registry.is_some_and(|r| r.trust_locked),
        }}));
    };
    let entry = entry?;
    let evaluator = style.evaluator();
    let question = mighty_run::questionnaire::guided::waiting(draft.requests).is_some();
    let plan = !question && draft.requests.iter().any(|r| r.state == "pending" && r.can_answer_plan);
    let pending_plan = draft.requests.iter().any(|r| r.state == "pending" && r.can_answer_plan);
    let phase = evaluator.effective_phase(guided_phase(entry, style, pane, pending_plan, &l), entry.starting_new);
    let busy = pane.status() == "running" || draft.starting;
    let job = evaluator.job_open(pane);
    let mut panel = serde_json::Map::new();
    if !style.manifest.phases.is_empty() {
        let mut ordered: Vec<&StylePhase> = style.manifest.phases.iter().collect();
        ordered.sort_by_key(|p| p.order);
        let phases: Vec<Value> = ordered
            .iter()
            .map(|item| {
                let state = if Some(&item.id) == phase.map(|p| &p.id) {
                    "current"
                } else if phase.is_some_and(|p| item.order <= p.order) {
                    "reached"
                } else {
                    "ahead"
                };
                json!({ "id": item.id, "title": item.title, "state": state })
            })
            .collect();
        panel.insert("phases".into(), Value::Array(phases));
    }
    if !question
        && let Some(action) = evaluator.rewrite_action(draft.text, phase, draft.attachments, busy, has_requests(pane), entry.starting_new)
    {
        let prompt = style.manifest.action(action).map(|a| a.prompt("")).unwrap_or_default();
        panel.insert("enter".into(), json!(mighty_styles::manifest::trim(&prompt)));
    }
    if question || plan {
        panel.insert("waiting".into(), json!(true));
        return Some(Value::Object(panel));
    }
    if let Some(p) = entry.prerequisites.as_ref().filter(|p| !p.ready) {
        panel.insert(
            "setup".into(),
            json!({ "missing": p.missing, "hint": p.hint.clone().filter(|h| !h.is_empty()), "installCommand": p.install_command }),
        );
    }
    let now = mighty_core::time::DateTimeOffset::now();
    let widgets: Vec<Value> = live(entry, style, pane, pending_plan, &l)
        .widgets
        .iter()
        .filter_map(|widget| match widget.kind.as_str() {
            "taskList" => {
                let tasks = presentation::tasks(&l, widget);
                (!tasks.is_empty()).then(|| {
                    let rows: Vec<Value> = tasks
                        .iter()
                        .map(|t| {
                            let elapsed = t.elapsed(&l, now);
                            let detail = [t.kind_title.clone(), t.status_title.clone(), elapsed]
                                .into_iter()
                                .filter(|s| !s.is_empty())
                                .collect::<Vec<_>>()
                                .join(" · ");
                            json!({ "text": t.text, "detail": detail, "running": t.running })
                        })
                        .collect();
                    json!({ "kind": "taskList", "tasks": rows })
                })
            }
            "progressBar" => presentation::progress(widget)
                .map(|(fraction, text)| json!({ "kind": "progressBar", "fraction": fraction.clamp(0.0, 1.0), "text": text })),
            "list" => {
                let items: Vec<String> =
                    widget.items.iter().flatten().map(|v| presentation::inline(v)).filter(|v| !v.is_empty()).take(20).collect();
                (!items.is_empty()).then(|| json!({ "kind": "list", "items": items }))
            }
            _ => widget.text.as_deref().filter(|t| !t.is_empty()).map(|t| json!({ "kind": "text", "text": presentation::inline(t) })),
        })
        .collect();
    if !widgets.is_empty() {
        panel.insert("widgets".into(), Value::Array(widgets));
    }
    let group =
        style.manifest.groups.iter().find(|g| Some(&g.id) == entry.group.as_ref()).or_else(|| evaluator.initial_group(&BTreeMap::new()));
    if evaluator.draws_group_map() {
        let groups: Vec<Value> = style
            .manifest
            .groups
            .iter()
            .map(|g| json!({ "id": g.id, "title": g.title, "axis": g.axis.clone().unwrap_or_default(), "help": g.question.clone().unwrap_or_else(|| g.title.clone()), "chosen": group.is_some_and(|c| c.id == g.id) }))
            .collect();
        panel.insert("groups".into(), Value::Array(groups));
        panel.insert("groupQuestion".into(), json!(group.and_then(|g| g.question.clone()).filter(|q| !q.is_empty())));
    }
    if busy && evaluator.draws_phase_progress() && !job {
        panel.insert("spinner".into(), json!(true));
    } else {
        let actions = evaluator.visible_actions(phase, group, busy, job, entry.starting_new);
        let recommended = evaluator.recommended_action(&BTreeMap::new());
        let prominent =
            (job || evaluator.at_start(phase) || evaluator.draws_phase_progress()).then(|| actions.first().map(|a| a.id.clone())).flatten();
        let chips: Vec<Value> = actions
            .iter()
            .map(|a| {
                let mark = a
                    .glyph
                    .clone()
                    .filter(|g| !g.is_empty())
                    .or_else(|| Some(presentation::icon(a.icon.as_deref()).to_string()).filter(|i| !i.is_empty()));
                json!({
                    "id": a.id, "title": a.title, "mark": mark,
                    "userInvoked": a.flags.iter().any(|f| f == "userInvoked"), "readOnly": a.flags.iter().any(|f| f == "readOnly"),
                    "emphasised": Some(&a.id) == prominent.as_ref() || Some(a.id.as_str()) == recommended,
                    "tint": tint(a.tint.as_deref()),
                    "enabled": !a.requires_text || !draft.text.chars().all(char::is_whitespace),
                    "help": presentation::action_help(&l, a),
                })
            })
            .collect();
        panel.insert("actions".into(), Value::Array(chips));
        panel.insert("catalogue".into(), json!(evaluator.draws_group_map()));
        if entry.starting_new {
            panel.insert("reset".into(), json!({ "kind": "cancel", "title": l.get("guidedPanel.cancelButton"), "help": Value::Null }));
        } else if !evaluator.at_start(phase)
            && let Some(title) = evaluator.reset_title()
        {
            panel.insert("reset".into(), json!({ "kind": "new", "title": title, "help": l.get("guidedPanel.resetHelp") }));
        }
    }
    if let Some(guidance) = evaluator.guidance(phase, busy, job) {
        panel.insert("guidance".into(), json!(guidance));
    }
    Some(Value::Object(panel))
}

/// The composer's style inputs for the pane (C# `RefreshStyles`): the chip and the guide panel.
pub fn window(host: &Host, view: &View, pane: &RunSession) -> (Option<StyleRow>, Option<Value>) {
    let ui = view.panes.get(pane.id());
    let starting = ui.is_some_and(|u| u.view.starting);
    let requests = crate::panes::requests(host, pane.id());
    let text = ui.map(|u| u.view.draft.clone()).unwrap_or_default();
    let draft = Draft { text: &text, attachments: ui.is_some_and(|u| !u.view.attachments.is_empty()), starting, requests: &requests };
    (row(host, &view.snapshot, pane, starting), guided(host, &view.snapshot, pane, &draft))
}

/// The cards' `styled` and `style_draws_tasks` (C# `activeStyle is not null`, `Evaluator.DrawsTasks`).
pub fn card_flags(pane: &RunSession) -> (bool, bool) {
    let all = panes();
    match all.get(pane.id()).and_then(|e| e.active.as_ref()) {
        Some(style) if pane.agent_view_mode.as_deref() == Some("mighty") => (true, style.evaluator().draws_tasks()),
        _ => (false, false),
    }
}

/// The style menu (C# `stylePicker.Flyout`): CLI, then every style this pane may see — its name, its source unless built in and, while
/// it waits for a yes or is blocked, why — the current one checked. None: only "CLI" (nothing read yet).
pub fn menu_rows(host: &Host, snapshot: &AppSnapshot, pane: &RunSession) -> Option<Vec<MenuRow>> {
    if !available(snapshot, pane) {
        return None;
    }
    let l = host.locale;
    let all = panes();
    let registry = all.get(pane.id()).and_then(|e| e.registry.clone())?;
    let mut rows = vec![MenuRow::Choice {
        text: "CLI".into(),
        enabled: true,
        checked: pane.mighty_style.is_none(),
        help: None,
        action: Some(MenuAction::Style(mighty_remote::phone::mighty::CLI_STYLE.into())),
    }];
    for candidate in &registry.styles {
        let count = |key: &str, n: usize| l.get_with(key, &[("count", &n.to_string())]);
        let why = match candidate.approval.as_str() {
            "pending" => format!(
                " · {} · {} · {}",
                presentation::state(&l, "pending"),
                count("settings.styles.actionCountTemplate", candidate.manifest.actions.len()),
                count("settings.styles.autoAllowCountTemplate", candidate.manifest.auto_allow().len())
            ),
            "revoked" => format!(" · {} · {}", presentation::state(&l, "revoked"), l.get("styles.menu.allowAgainInSettings")),
            _ => String::new(),
        };
        rows.push(MenuRow::Choice {
            text: format!("{}{why}", presentation::name(&l, candidate)),
            enabled: candidate.approval != "revoked" && (!registry.trust_locked || candidate.runnable()),
            checked: Some(candidate.id()) == pane.mighty_style.as_deref(),
            help: Some(candidate.manifest.summary().to_string()),
            action: Some(MenuAction::Style(candidate.id().to_string())),
        });
    }
    Some(rows)
}

fn registered(id: &str, style: &str) -> Option<RegisteredStyle> {
    panes().get(id).and_then(|e| e.registry.as_ref()).and_then(|r| r.styles.iter().find(|s| s.id() == style).cloned())
}

/// C# `ChooseStyle(selected)`: a style not yet said yes to asks first (the approval sheet), then the pane points at it; `cli` is none.
pub fn choose(host: &Arc<Host>, id: &str, style: &str) {
    let (host, id, style) = (host.clone(), id.to_string(), style.to_string());
    crate::native::act(&host.clone(), async move { choose_now(&host, &id, &style).await });
}

async fn choose_now(host: &Arc<Host>, id: &str, style: &str) -> Result<(), String> {
    let before = {
        let view = host.view();
        let Some(pane) = view.snapshot.session(id) else { return Ok(()) };
        if pane.status() == "running" || view.panes.get(id).is_some_and(|u| u.view.starting) {
            return Ok(());
        }
        pane.mighty_style.clone()
    };
    let selected = if style == mighty_remote::phone::mighty::CLI_STYLE { None } else { registered(id, style) };
    if style != mighty_remote::phone::mighty::CLI_STYLE && selected.is_none() {
        return Err(approval_required(host));
    }
    if let Some(s) = &selected
        && !s.runnable()
        && !approve(host, s).await?
    {
        return Ok(());
    }
    let workspace = {
        let view = host.view();
        let Some(pane) = view.snapshot.session(id) else { return Ok(()) };
        if pane.status() == "running" || pane.mighty_style != before {
            return Ok(());
        }
        local_workspace(&view.snapshot, pane)
    };
    let selected = match selected {
        None => None,
        Some(s) => {
            let (reader, folder) = (host.clone(), workspace.clone().unwrap_or_else(|| host.data_dir.clone()));
            let id_of = s.id().to_string();
            let hash = s.hash.clone();
            let fresh =
                tauri::async_runtime::spawn_blocking(move || load_registry(&reader, &folder).runnable(Some(&id_of), Some(&hash)).cloned())
                    .await
                    .map_err(|e| e.to_string())?;
            match fresh {
                Some(fresh) if StyleRegistry::unchanged(&fresh) => Some(fresh),
                _ => return Err(approval_required(host)),
            }
        }
    };
    {
        let mut view = host.view();
        crate::composer::change(&mut view, id, |p| {
            let since = if p.mighty_style.as_deref() == selected.as_ref().map(|s| s.id()) { p.mighty_style_since.clone() } else { None };
            RunSession {
                mighty_style: selected.as_ref().map(|s| s.id().to_string()),
                mighty_style_hash: selected.as_ref().map(|s| s.hash.clone()),
                mighty_style_since: since,
                ..p.clone()
            }
        })?;
    }
    if let Some(entry) = panes().get_mut(id) {
        entry.starting_new = false;
        entry.group = None;
        entry.loaded_key = None;
    }
    host.schedule();
    reload(host, id, false);
    Ok(())
}

/// The approval sheet, then the decision saved (C# `ShowStyleApproval` + `ApproveStyle`): false when it was not approved. A build that
/// saves nothing shows the sheet to read only.
async fn approve(host: &Arc<Host>, style: &RegisteredStyle) -> Result<bool, String> {
    let writes = host.writes();
    if !writes {
        host.log.line("styles: an approval is not stored (no --allow-state-write)");
    }
    if !show_approval(host, style, !writes).await {
        return Ok(false);
    }
    let (writer, chosen) = (host.clone(), style.clone());
    tauri::async_runtime::spawn_blocking(move || {
        if !StyleRegistry::unchanged(&chosen) {
            return Err(approval_required(&writer));
        }
        trust(&writer).decide(&chosen, "approved").map_err(|e| e.0)
    })
    .await
    .map_err(|e| e.to_string())??;
    Ok(true)
}

// ── the sheets (D11–D13) ────────────────────────────────────────────────────────────────────────────────────────────────────

/// D11, C# `ShowStyleApproval` (MainWindow.StyleApproval.cs): the style's name and source, the risk-bearing sections first (origin,
/// auto-allowed tools, install, Enter), the rest and the raw manifest; approval needs a second press when the style allows tools by
/// itself, and none while the trust store is locked. True when approved (never for `read_only`).
pub async fn show_approval(host: &Arc<Host>, style: &RegisteredStyle, read_only: bool) -> bool {
    use mighty_core::design_tokens::{Token, metrics};
    use mighty_native::kit::TextStyle;
    use mighty_native::surface::{DefaultButton, DialogButton, DialogChrome, DialogResult, DialogSpec};
    use mighty_native::ui::{Content, El, Local};
    let l = host.locale;
    let store = trust(host);
    let locked = {
        let store = store.clone();
        tauri::async_runtime::spawn_blocking(move || store.load().is_err()).await.unwrap_or(true)
    };
    let sections = presentation::approval(&l, style);
    let automatic = style.manifest.auto_allow().len();
    let message = Local::new(if locked {
        format!("{}\n{}", l.get("settings.styles.lockBanner"), store.file_path().display())
    } else {
        String::new()
    });
    let (name, source) = (style.manifest.name().to_string(), (style.source != "bundled").then(|| presentation::source(&l, &style.source)));
    let raw = String::from_utf8_lossy(&style.bytes).into_owned();
    let shown = message.clone();
    let mut spec = DialogSpec::new(name.clone(), move |ui| {
        let mut parts = vec![];
        let mut head = vec![ui.settings_text(name.clone(), TextStyle::size(15.0).medium())];
        if let Some(source) = &source {
            head.push(ui.settings_capsule(source.clone(), ui.brush(Token::Ink2), ui.theme().subtle(), 9.0, 8.0));
        }
        parts.push(El::stack(true, metrics::SPACING_SM, head).aid("style-approval-heading"));
        for section in &sections {
            let mut lines = vec![ui.settings_text(section.title.clone(), TextStyle::size(12.0).medium())];
            for line in &section.lines {
                let style = TextStyle::size(11.0).ink(Token::Ink2).selectable();
                lines.push(ui.settings_text(line.clone(), if section.monospaced { style.mono() } else { style }));
            }
            parts.push(El::stack(false, metrics::SPACING_XS, lines).aid(format!("style-approval-{}", section.id)));
        }
        parts.push(
            El::stack(
                false,
                metrics::SPACING_XS,
                vec![
                    ui.settings_text(l.get("styles.approval.raw"), TextStyle::size(12.0).medium()),
                    ui.settings_text(raw.clone(), TextStyle::size(10.0).ink(Token::Ink2).mono().selectable()),
                ],
            )
            .aid("style-approval-raw"),
        );
        let said = shown.get();
        if !said.is_empty() {
            parts.push(ui.settings_text(said, TextStyle::size(11.0).ink(Token::WaitText).selectable()).aid("style-approval-message"));
        }
        Content::El(Box::new(El::stack(false, 14.0, parts).aid("style-approval-sheet")))
    })
    .styled(Some(640.0), Some(560.0));
    spec.close = Some(l.get(if read_only { "settings.closeButton" } else { "guidedPanel.cancelButton" }));
    spec.default_button = DefaultButton::Close;
    spec.chrome = Some(DialogChrome { panel: Some("styleApproval".into()), icon: Some("palette".into()), ..DialogChrome::default() });
    if !read_only {
        spec.primary = Some(DialogButton::new(l.get("settings.styles.allowButton")).enabled_when(move || !locked));
        let mut decision = presentation::StyleApprovalDecision::new(automatic);
        spec.accept = Some(Box::new(move |answer| {
            if answer != DialogResult::Primary {
                return true;
            }
            if locked {
                return false;
            }
            if decision.press(true) {
                return true;
            }
            if decision.confirming {
                message.set(format!(
                    "{}\n{}",
                    l.get_with("styles.approval.secondConfirmation", &[("count", &automatic.to_string())]),
                    l.get("styles.approval.confirmAgain")
                ));
            }
            false
        }));
    }
    let answer = crate::native::show_dialog(host, spec).await;
    answer == DialogResult::Primary && !read_only
}

/// D12, C# `BuildStylesSection`'s remove alert (MainWindow.Styles.cs:113-124): the file's path, danger; true when confirmed.
async fn confirm_remove(host: &Arc<Host>, path: &str) -> bool {
    use mighty_native::surface::{DefaultButton, DialogButton, DialogChrome, DialogResult, DialogSpec};
    use mighty_native::ui::Content;
    let l = host.locale;
    let text = path.to_string();
    let mut spec = DialogSpec::new(l.get("settings.styles.removeButton"), move |_| Content::Text(text.clone())).styled(None, None);
    spec.primary = Some(DialogButton::new(l.get("settings.styles.removeButton")));
    spec.close = Some(l.get("guidedPanel.cancelButton"));
    spec.default_button = DefaultButton::Close;
    spec.chrome = Some(DialogChrome { danger: true, ..DialogChrome::default() });
    crate::native::show_dialog(host, spec).await == DialogResult::Primary
}

/// D13, C# `InstallStyle`'s confirmation (MainWindow.Styles.cs:357-376): the exact command the terminal will be given, read only.
async fn confirm_install(host: &Arc<Host>, command: &str) -> bool {
    use mighty_native::surface::{DefaultButton, DialogButton, DialogResult, DialogSpec};
    use mighty_native::ui::{Content, El};
    let l = host.locale;
    let text = command.to_string();
    let mut spec = DialogSpec::new(l.get("guidedPanel.installButton"), move |_| {
        Content::El(Box::new(El::text_box(text.clone()).aid("style-install-command")))
    })
    .styled(None, None);
    spec.primary = Some(DialogButton::new(l.get("guidedPanel.installButton")));
    spec.close = Some(l.get("guidedPanel.cancelButton"));
    spec.default_button = DefaultButton::Close;
    crate::native::show_dialog(host, spec).await == DialogResult::Primary
}

// ── guidedAction ────────────────────────────────────────────────────────────────────────────────────────────────────────────

/// The page method this part answers: `guidedAction{paneId, what, id}` (C# `WebGuided`, MainWindow.WebShell.Guided.cs).
pub fn register(methods: &mut Registry) {
    methods.add("guidedAction", |host: Arc<Host>, args| async move {
        let id = {
            let mut view = host.view();
            crate::panes::ensure(&mut view, crate::methods::text(&args, "paneId"))?
        };
        let what = crate::methods::text(&args, "what").unwrap_or_default().to_string();
        let target = crate::methods::text(&args, "id").map(str::to_string);
        guided_action(&host, &id, &what, target.as_deref()).await?;
        Ok(Value::Null)
    });
}

/// One press of the guide panel.
pub async fn guided_action(host: &Arc<Host>, id: &str, what: &str, target: Option<&str>) -> Result<(), String> {
    match what {
        "action" => invoke(host, id, target.unwrap_or_default()).await,
        "group" => {
            if let Some(entry) = panes().get_mut(id) {
                entry.group = target.map(str::to_string);
            }
            mark(host, id);
            Ok(())
        }
        "reset" => {
            if let Some(entry) = panes().get_mut(id) {
                entry.starting_new = target == Some("new");
            }
            mark(host, id);
            Ok(())
        }
        "recheck" => {
            reload(host, id, false);
            Ok(())
        }
        "install" => install(host, id).await,
        "view" => {
            let style = host.view().snapshot.session(id).and_then(|p| p.mighty_style.clone());
            match style {
                Some(style) => choose_now(host, id, &style).await,
                None => Ok(()),
            }
        }
        _ => Err("Unknown guided action.".into()),
    }
}

/// C# `InvokeStyleAction`: the action's prompt put in the editor and sent as the app's own (never asked of another CLI); the draft
/// comes back unless something else was typed meanwhile.
async fn invoke(host: &Arc<Host>, id: &str, action: &str) -> Result<(), String> {
    let style = panes().get(id).and_then(|e| e.active.clone()).ok_or_else(|| approval_required(host))?;
    let unchanged = {
        let style = style.clone();
        tauri::async_runtime::spawn_blocking(move || StyleRegistry::unchanged(&style)).await.unwrap_or(false)
    };
    if !unchanged {
        return Err(approval_required(host));
    }
    let action = style.manifest.action(action).cloned().ok_or_else(|| host.locale.get("remote.error.unknownSkill"))?;
    let original = {
        let mut view = host.view();
        if crate::panes::requests(host, id).iter().any(|p| p.state == "pending" && p.can_answer_questions) {
            return Ok(());
        }
        let ui = crate::panes::ui(&mut view, id)?;
        if action.requires_text && ui.view.draft.chars().all(char::is_whitespace) {
            return Ok(());
        }
        let original = ui.view.draft.clone();
        ui.view.draft = action.prompt(if action.takes_text { &original } else { "" });
        ui.dirty = true;
        original
    };
    let prompt = action.prompt(if action.takes_text { &original } else { "" });
    let sent = crate::composer::submit_own(host, id).await;
    {
        let mut view = host.view();
        if let Some(ui) = view.panes.get_mut(id)
            && (ui.view.draft == prompt || (!action.takes_text && ui.view.draft.is_empty()))
        {
            ui.view.draft = original.clone();
            ui.dirty = true;
            let _ = crate::composer::change(&mut view, id, |p| RunSession { draft: Some(original.clone()), ..p.clone() });
        }
    }
    sent.map(|_| ())
}

/// C# `InstallStyle`: the install command, confirmed (D13), typed into a new shell pane; only where the host may change the machine.
async fn install(host: &Arc<Host>, id: &str) -> Result<(), String> {
    let style = panes().get(id).and_then(|e| e.active.clone()).ok_or_else(|| approval_required(host))?;
    let Some(command) = style.manifest.root.get("install").and_then(|i| mighty_styles::manifest::text(i, "command")).map(str::to_string)
    else {
        return Ok(());
    };
    if !host.may_change_machine() {
        host.log.line("styles: the install was refused (no --allow-state-write, or a smoke)");
        return Ok(());
    }
    let workspace = {
        let view = host.view();
        view.snapshot.session(id).and_then(|p| local_workspace(&view.snapshot, p))
    }
    .ok_or_else(|| approval_required(host))?;
    let still = |host: &Arc<Host>, style: &RegisteredStyle, workspace: PathBuf| {
        let (reader, style) = (host.clone(), style.clone());
        tauri::async_runtime::spawn_blocking(move || {
            StyleRegistry::unchanged(&style) && load_registry(&reader, &workspace).runnable(Some(style.id()), Some(&style.hash)).is_some()
        })
    };
    if !still(host, &style, workspace.clone()).await.unwrap_or(false) {
        return Err(approval_required(host));
    }
    let shell = crate::terminal::terminals(host)?.default_shell();
    let actual = permissions::install_command(&command, &shell.to_string_lossy());
    if !confirm_install(host, &actual).await {
        return Ok(());
    }
    let moved = {
        let view = host.view();
        view.snapshot.session(id).is_none_or(|p| {
            p.mighty_style.as_deref() != Some(style.id())
                || p.mighty_style_hash.as_deref() != Some(&style.hash)
                || view.snapshot.active_workspace_id.as_deref() != Some(p.workspace_id())
        })
    };
    if moved || !still(host, &style, workspace).await.unwrap_or(false) {
        return Err(approval_required(host));
    }
    let title = style
        .manifest
        .root
        .get("install")
        .and_then(|i| mighty_styles::manifest::text(i, "paneTitle"))
        .unwrap_or(style.manifest.name())
        .to_string();
    run_terminal_command(host, &actual, &format!("{title} · {}", presentation::name(&host.locale, &style)), false).await
}

/// C# `RunTerminalCommandAsync(command, title, autoRun)` (MainWindow.Terminal.cs:20-42): a new shell pane titled `title`, and the
/// command typed into it once its shell is up (run when `auto_run`); refused for a command with a line break, and with
/// `terminal.failed` when the pane went or no shell came within 20 s.
pub async fn run_terminal_command(host: &Arc<Host>, command: &str, title: &str, auto_run: bool) -> Result<(), String> {
    use mighty_platform::pty::TerminalInput;
    let l = host.locale;
    if TerminalInput::has_line_break(command) {
        return Err(l.get("terminal.input.newlineRefused"));
    }
    let pane_id = mighty_core::wire::new_id();
    let (made, named) = (pane_id.clone(), title.to_string());
    if !crate::frame::add_pane(host, "shell", "claude", None, move |pane| RunSession { id: Some(made), title: Some(named), ..pane })? {
        return Ok(());
    }
    host.schedule();
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    while std::time::Instant::now() < deadline {
        if host.view().snapshot.session(&pane_id).is_none() {
            return Err(l.get("terminal.failed"));
        }
        let terminal = crate::terminal::terminals(host)?.pane(host, Some(&pane_id))?;
        let mut written = None;
        let typed = TerminalInput::apply(command, auto_run, |text| {
            let answer = terminal.write(text);
            written = written.or(answer.map(|_| ()));
            answer.unwrap_or(false)
        });
        if written.is_some() {
            return match typed {
                TerminalInput::Refused => Err(l.get("terminal.input.newlineRefused")),
                TerminalInput::Failed => Err(l.get("terminal.inputBusy")),
                _ => Ok(()),
            };
        }
        if terminal.is_closed() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    Err(l.get("terminal.failed"))
}

// ── sending ─────────────────────────────────────────────────────────────────────────────────────────────────────────────────

/// C# `PrepareStyleSubmission(draft, attachments)`: a bare first draft rewritten to the style's slash command, approval checked again
/// off the dispatcher; the style's start time set at the first request (or a reset's).
pub async fn prepare_submission(host: &Arc<Host>, id: &str, draft: &str, attachments: bool) -> Result<String, String> {
    let (pane, workspace) = {
        let view = host.view();
        let Some(pane) = view.snapshot.session(id).cloned() else { return Ok(draft.to_string()) };
        let workspace = local_workspace(&view.snapshot, &pane);
        (pane, workspace)
    };
    let Some(workspace) = workspace.filter(|_| permissions::styled(&pane)) else { return Ok(draft.to_string()) };
    let reader = host.clone();
    let chosen = pane.clone();
    let style = tauri::async_runtime::spawn_blocking(move || {
        let registry = load_registry(&reader, &workspace);
        registry.runnable(chosen.mighty_style.as_deref(), chosen.mighty_style_hash.as_deref()).cloned().filter(StyleRegistry::unchanged)
    })
    .await
    .map_err(|e| e.to_string())?
    .ok_or_else(|| approval_required(host))?;
    let pending_plan = crate::panes::requests(host, id).iter().any(|r| r.state == "pending" && r.can_answer_plan);
    let (text, starting_new) = {
        let all = panes();
        let empty = PaneStyle::default();
        let entry = all.get(id).unwrap_or(&empty);
        let evaluator = style.evaluator();
        let phase = evaluator.effective_phase(guided_phase(entry, &style, &pane, pending_plan, &host.locale), entry.starting_new);
        let action =
            evaluator.rewrite_action(draft, phase, attachments, pane.status() == "running", has_requests(&pane), entry.starting_new);
        (action.and_then(|a| style.manifest.action(a)).map_or_else(|| draft.to_string(), |a| a.prompt(draft)), entry.starting_new)
    };
    if pane.mighty_style_since.is_none() || starting_new {
        let mut view = host.view();
        crate::composer::change(&mut view, id, |p| RunSession { mighty_style_since: Some(mighty_core::time::now_o()), ..p.clone() })?;
    }
    if let Some(entry) = panes().get_mut(id) {
        entry.starting_new = false;
    }
    Ok(text)
}

/// C# `PrepareStyleRunRequest`'s style half (`StyleRunPermissions.Bind`): the style's grants and its plan-mode launch, the approval
/// read again; a remote pane's request goes as it is.
pub async fn prepare_run(host: &Arc<Host>, id: &str, request: StartRunRequest) -> Result<StartRunRequest, String> {
    let (pane, workspace) = {
        let view = host.view();
        let Some(pane) = view.snapshot.session(id).cloned() else { return Ok(request) };
        let workspace = local_workspace(&view.snapshot, &pane);
        (pane, workspace)
    };
    let Some(workspace) = workspace.filter(|_| permissions::styled(&pane)) else { return Ok(request) };
    let (profile, locale, chosen) = (host.data_dir.clone(), host.locale, pane.clone());
    let prepared =
        tauri::async_runtime::spawn_blocking(move || permissions::bind(request, &chosen, &profile, &workspace, access(), locale))
            .await
            .map_err(|e| e.to_string())??;
    let moved = host
        .view()
        .snapshot
        .session(id)
        .is_none_or(|now| now.mighty_style != pane.mighty_style || now.mighty_style_hash != pane.mighty_style_hash);
    if moved {
        return Err(approval_required(host));
    }
    Ok(prepared)
}

/// C# `DesktopService.ReceiveToolPermission`'s style half: true when the pane is shown `request`; a request its style allows by itself
/// is answered allowed here, once, and its settling is no news.
pub fn tool_permission(host: &Arc<Host>, request: &ToolPermissionRequest) -> bool {
    ALLOWING.receive(
        request,
        || {
            let (pane, workspace) = {
                let view = host.view();
                let pane = view.snapshot.session(&request.run_id).cloned();
                let workspace = pane.as_ref().and_then(|p| view.snapshot.workspace(p.workspace_id())).map(|w| PathBuf::from(w.path()));
                (pane, workspace)
            };
            let Some(workspace) = workspace else { return false };
            permissions::auto_allows(request, pane.as_deref(), &host.data_dir, &workspace, access(), host.locale)
        },
        || {
            let (answer, run, id) = (host.clone(), request.run_id.clone(), request.id.clone());
            tauri::async_runtime::spawn(async move {
                let failed = match answer.runs() {
                    Some(runs) => runs.service.respond_to_tool_permission(&run, &id, true).await.is_err(),
                    None => true,
                };
                // Nothing settled it: it waits for the user like any other.
                if failed && ALLOWING.release(&run, &id) {
                    mark(&answer, &run);
                }
            });
            true
        },
    )
}

// ── the Mighty view and the phone ───────────────────────────────────────────────────────────────────────────────────────────

/// What the Mighty view reads of the pane's style (C# `activeStyle`, `GuidedPhase`, `EffectivePhase`, `styleRegistry`).
pub struct Source {
    pub style: RegisteredStyle,
    pub phase: Option<StylePhase>,
    pub effective: Option<StylePhase>,
    pub starting_new: bool,
    pub registry: Arc<StyleRegistry>,
}

impl Source {
    /// `StylePresentation.RequestPrefix(registry.Styles, input)`.
    pub fn request_prefix(&self, input: &str) -> Option<String> {
        presentation::request_prefix(&self.registry.styles, input)
    }
}

/// The pane's style for its Mighty view and the phone's structure, when it has one.
pub fn source(host: &Host, pane: &RunSession) -> Option<Source> {
    if pane.agent_view_mode.as_deref() != Some("mighty") {
        return None;
    }
    let pending_plan = crate::panes::requests(host, pane.id()).iter().any(|r| r.state == "pending" && r.can_answer_plan);
    let all = panes();
    let entry = all.get(pane.id())?;
    let style = entry.active.clone()?;
    let phase = guided_phase(entry, &style, pane, pending_plan, &host.locale).cloned();
    let effective = style.evaluator().effective_phase(phase.as_ref(), entry.starting_new).cloned();
    Some(Source { phase, effective, starting_new: entry.starting_new, registry: entry.registry.clone()?, style })
}

/// C# `MobileActiveStyle` / `MobileRunningStyle`: the style the pane really runs (its read started when the pane moved).
pub fn active_style(host: &Host, pane: &RunSession) -> Option<String> {
    let snapshot = host.view().snapshot.clone();
    ensure(host, &snapshot, pane);
    if !available(&snapshot, pane) {
        return None;
    }
    panes()
        .get(pane.id())
        .and_then(|e| e.active.as_ref())
        .filter(|s| Some(s.id()) == pane.mighty_style.as_deref())
        .map(|s| s.id().to_string())
}

/// The styles a pane may pick on the phone (C# `MobileOptions`' styles): none outside the Mighty view or for a remote pane.
pub fn offered(host: &Host, snapshot: &AppSnapshot, pane: &RunSession, view_mode: &str) -> Vec<RegisteredStyle> {
    let Some(workspace) = local_workspace(snapshot, pane) else { return Vec::new() };
    if !mighty_remote::phone::mighty::guided_styles_available(pane.kind(), pane.provider(), view_mode) {
        return Vec::new();
    }
    let cached = panes().get(pane.id()).and_then(|e| e.registry.clone());
    if let Some(registry) = cached {
        return registry.styles.clone();
    }
    load_registry(host, &workspace).styles
}

/// The phone's `panel`, `ouroboros` and the mighty digest's style part (C# `MobileStylePanel`, `MobileLegacyStyleAdapter.Payload`).
pub struct PhoneStyle {
    pub panel: Value,
    pub ouroboros: Option<Value>,
    pub digest: String,
}

pub fn phone(host: &Host, pane: &RunSession, busy: bool) -> Option<PhoneStyle> {
    let pending_plan = crate::panes::requests(host, pane.id()).iter().any(|r| r.state == "pending" && r.can_answer_plan);
    let all = panes();
    let entry = all.get(pane.id())?;
    let style = entry.active.as_ref().filter(|s| Some(s.id()) == pane.mighty_style.as_deref())?;
    let reading = live(entry, style, pane, pending_plan, &host.locale);
    let prerequisites = entry.prerequisites.clone().unwrap_or_default();
    let panel = mighty_styles::projection::make(
        style,
        &mighty_styles::projection::session_prompts(pane),
        entry.group.as_deref(),
        &BTreeMap::new(),
        &prerequisites,
        busy,
        Some(pane),
        Some(&reading),
    );
    let phase = guided_phase(entry, style, pane, pending_plan, &host.locale);
    let ouroboros = mighty_remote::phone::legacy_style::payload(style, phase, prerequisites.ready).map(|o| o.to_value());
    let widgets: Vec<Value> = reading.widgets.iter().map(presentation::payload).collect();
    let digest = format!(
        "{}|{}|{}|{}|{}|{}|{}",
        style.id(),
        style.hash,
        phase.map(|p| p.id.as_str()).unwrap_or(""),
        pane.status() == "running",
        prerequisites.ready,
        prerequisites.missing.join("\n"),
        serde_json::to_string(&widgets).unwrap_or_default()
    );
    Some(PhoneStyle { panel: panel.to_value(), ouroboros, digest })
}

/// The style a phone's settings leave the pane with: the style, its hash and whether it changed.
pub type PhoneChoice = (Option<String>, Option<String>, bool);

/// C# `MobileSettings`' style half: the style the pane ends up with, its hash and whether it changed (`StyleAfter`), refused with
/// `remote.error.unknownStyle` (400) or "Style changed." (409).
pub fn phone_choice(
    host: &Host,
    snapshot: &AppSnapshot,
    pane: &RunSession,
    body: &serde_json::Map<String, Value>,
) -> Result<PhoneChoice, (u16, String)> {
    let l = host.locale;
    let text = |k: &str| body.get(k).and_then(Value::as_str);
    let shown = active_style(host, pane).unwrap_or_else(|| mighty_remote::phone::mighty::CLI_STYLE.to_string());
    let requested = text("styleId").or_else(|| text("mightyStyle"));
    let styles = offered(host, snapshot, pane, "mighty");
    if let Some(requested) = requested {
        let known = if text("styleId").is_some() {
            mighty_remote::phone::legacy_style::style_options(&styles).iter().any(|s| s.id == requested)
        } else {
            mighty_remote::phone::legacy_style::legacy_style_options(&styles).iter().any(|s| s.id == requested)
        };
        if !known {
            return Err((400, l.get("remote.error.unknownStyle")));
        }
    }
    let style = mighty_remote::phone::mighty::style_after(requested, &shown, pane.mighty_style.as_deref());
    let changes = requested.is_some_and(|r| r != shown);
    let mut hash = pane.mighty_style_hash.clone();
    if changes {
        hash = None;
        if let Some(style) = &style {
            let workspace = local_workspace(snapshot, pane).ok_or_else(|| (400, l.get("remote.error.unknownStyle")))?;
            let registry = load_registry(host, &workspace);
            let selected =
                registry.runnable(Some(style), None).or_else(|| registry.styles.iter().find(|s| s.id() == style && s.runnable()));
            let selected = selected.ok_or_else(|| (400, l.get("remote.error.unknownStyle")))?;
            if !StyleRegistry::unchanged(selected) {
                return Err((409, "Style changed.".into()));
            }
            hash = Some(selected.hash.clone());
        }
    }
    Ok((if changes { style } else { pane.mighty_style.clone() }, hash, changes))
}

/// C# `MobileGuided`'s checks: the prompt the phone's guided press sends, refused with the router's statuses and words.
pub fn phone_guided(
    host: &Host,
    pane: &RunSession,
    request: &mighty_remote::phone::mighty::MobileGuidedRequest,
) -> Result<String, (u16, String)> {
    let l = host.locale;
    let snapshot = host.view().snapshot.clone();
    let workspace = local_workspace(&snapshot, pane).ok_or_else(|| (400, l.get("remote.error.unknownStyle")))?;
    let registry = load_registry(host, &workspace);
    if pane.provider() != "claude" || !registry.styles.iter().any(|s| s.id() == request.style && s.runnable()) {
        return Err((400, l.get("remote.error.unknownStyle")));
    }
    let style = registry
        .runnable(pane.mighty_style.as_deref(), pane.mighty_style_hash.as_deref())
        .filter(|_| pane.agent_view_mode.as_deref() == Some("mighty") && Some(request.style.as_str()) == pane.mighty_style.as_deref())
        .ok_or_else(|| (409, l.get_with("remote.error.otherStyle", &[("style", &request.style)])))?;
    let action = style.manifest.action(&request.action).ok_or_else(|| (400, l.get("remote.error.unknownSkill")))?;
    if action.requires_text && request.text.chars().all(char::is_whitespace) {
        return Err((400, "This action needs text.".into()));
    }
    if !StyleRegistry::unchanged(style) {
        return Err((409, "Style changed.".into()));
    }
    let text = if action.takes_text { mighty_remote::phone::mighty::single_line(&request.text) } else { String::new() };
    Ok(action.prompt(&text))
}

// ── the watcher ─────────────────────────────────────────────────────────────────────────────────────────────────────────────

static WATCHERS: LazyLock<Mutex<HashMap<PathBuf, StyleChangeWatcher>>> = LazyLock::new(|| Mutex::new(HashMap::new()));
static STARTED: Mutex<Option<Arc<Host>>> = Mutex::new(None);

/// C# `WatchStyles`/`PruneStyleWatchers`: the profile's `styles` and `style-trust` every 500 ms, and for each workspace a pane shows
/// its style rows in, its `.claude\mighty-styles` and the active style's source folders gathered for 2 s; off in a smoke.
fn watch(host: &Arc<Host>) {
    if host.smoke.is_some() || STARTED.lock().map(|s| s.is_none()).unwrap_or(true) {
        return;
    }
    let mut wanted: HashMap<PathBuf, (Option<PathBuf>, Duration)> = HashMap::new();
    for folder in [host.data_dir.join("styles"), host.data_dir.join("style-trust")] {
        wanted.insert(folder, (None, Duration::from_millis(500)));
    }
    for entry in panes().values() {
        let Some(workspace) = &entry.workspace else { continue };
        let mut folders = vec![workspace.join(".claude").join("mighty-styles")];
        if let Some(sources) = entry.active.as_ref().and_then(|s| s.manifest.root.get("stateSources")) {
            for source in mighty_styles::manifest::items(sources, "files") {
                let path = mighty_styles::manifest::text(source, "path").unwrap_or_default().replace('\\', "/");
                let parts: Vec<&str> = path.split('/').filter(|p| !p.is_empty()).collect();
                let mut leading: Vec<&str> = parts.iter().take_while(|p| !p.contains('*') && !p.contains('?')).copied().collect();
                if leading.len() == parts.len() {
                    leading.pop();
                }
                if !leading.is_empty() {
                    folders.push(leading.iter().fold(workspace.clone(), |at, part| at.join(part)));
                }
            }
        }
        for folder in folders {
            wanted.insert(folder, (Some(workspace.clone()), Duration::from_secs(2)));
        }
    }
    let mut watchers = WATCHERS.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    watchers.retain(|folder, _| wanted.contains_key(folder));
    for (folder, (workspace, latency)) in wanted {
        if watchers.contains_key(&folder) {
            continue;
        }
        let owner = host.clone();
        let watcher = StyleChangeWatcher::new(
            &folder,
            move |changed| {
                let ids: Vec<String> = panes()
                    .iter()
                    .filter(|(_, e)| match &workspace {
                        None => true,
                        Some(w) => {
                            e.workspace.as_ref() == Some(w)
                                && mighty_styles::watcher::concerns(&changed, w, e.active.as_ref().map(|s| &*s.manifest))
                        }
                    })
                    .map(|(id, _)| id.clone())
                    .collect();
                for id in ids {
                    reload(&owner, &id, false);
                }
            },
            Some(latency),
        );
        watchers.insert(folder, watcher);
    }
}

/// After the run engine (main.rs): the change watcher (never in a smoke).
pub fn start(host: &Arc<Host>, _platform: &mighty_platform::Platform) {
    if host.smoke.is_some() {
        return;
    }
    if let Ok(mut started) = STARTED.lock() {
        *started = Some(host.clone());
    }
    watch(host);
}

/// At quit (main.rs), before the runs are disposed: every watch stopped.
pub fn shutdown(_host: &Arc<Host>) {
    if let Ok(mut started) = STARTED.lock() {
        *started = None;
    }
    let drained: Vec<StyleChangeWatcher> = WATCHERS.lock().map(|mut w| w.drain().map(|(_, v)| v).collect()).unwrap_or_default();
    drop(drained);
}

// ── the settings box's work ─────────────────────────────────────────────────────────────────────────────────────────────────

/// The settings box's workspace: the active one's folder on this computer, else the profile (C# `?? StateDirectory`).
pub fn settings_workspace(host: &Host) -> PathBuf {
    let view = host.view();
    let snapshot = &view.snapshot;
    snapshot
        .active_workspace_id
        .as_deref()
        .and_then(|id| snapshot.workspace(id))
        .filter(|w| w.remote.is_none() && !w.path().is_empty())
        .map_or_else(|| host.data_dir.clone(), |w| PathBuf::from(w.path()))
}

/// The registry as the settings box reads it.
pub async fn settings_registry(host: &Arc<Host>) -> StyleRegistry {
    let (reader, workspace) = (host.clone(), settings_workspace(host));
    tauri::async_runtime::spawn_blocking(move || load_registry(&reader, &workspace)).await.unwrap_or_default()
}

/// Every pane reads its styles again (a decision or a file changed from the settings box).
pub fn reload_all(host: &Arc<Host>) {
    let ids: Vec<String> = panes().keys().cloned().collect();
    for id in ids {
        if let Some(entry) = panes().get_mut(&id) {
            entry.loaded_key = None;
        }
        reload(host, &id, false);
    }
}

/// C#'s register button: the picked manifest read, decoded and approved (D11), then copied to `<data>\styles\<id>.json` and its
/// approval saved; nothing is copied when the sheet is cancelled.
pub async fn register_file(host: &Arc<Host>, file: PathBuf) -> Result<(), String> {
    if !host.writes() {
        host.log.line("styles: a registration is not stored (no --allow-state-write)");
        return Ok(());
    }
    let folder = file.parent().map(Path::to_path_buf).unwrap_or_default();
    let name = file.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let bytes = tauri::async_runtime::spawn_blocking(move || mighty_styles::files::read(&folder, &name, decoder::MAXIMUM_BYTES))
        .await
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "Cannot read style manifest.".to_string())?;
    let manifest = decoder::decode(&bytes, "user", None).map_err(|e| e.message)?;
    let candidate = RegisteredStyle::decoded(manifest, "user", &file.to_string_lossy(), None, "pending", &bytes);
    if !show_approval(host, &candidate, false).await {
        return Ok(());
    }
    let (writer, workspace) = (host.clone(), settings_workspace(host));
    tauri::async_runtime::spawn_blocking(move || -> Result<(), String> {
        let l = writer.locale;
        let folder = writer.data_dir.join("styles");
        std::fs::create_dir_all(&folder).map_err(|e| e.to_string())?;
        let profile_root = mighty_core::workspace_files::real_path(&writer.data_dir);
        let styles_root = mighty_core::workspace_files::real_path(&folder);
        let linked = matches!(mighty_core::workspace_files::link_target(&folder), Ok(Some(_)));
        let (Some(profile_root), Some(styles_root)) = (profile_root, styles_root) else {
            return Err(l.get("guidedPanel.approvalRequired"));
        };
        if linked || !mighty_core::workspace_files::contains(&styles_root, &profile_root) {
            return Err(l.get("guidedPanel.approvalRequired"));
        }
        let registry = load_registry(&writer, &workspace);
        if registry.trust_locked {
            return Err(l.get("settings.styles.lockBanner"));
        }
        if registry.styles.iter().any(|s| s.id() == candidate.id()) {
            return Err(l.get("styles.registration.duplicate"));
        }
        let destination = styles_root.join(format!("{}.json", candidate.id()));
        {
            use std::io::Write;
            let mut output = std::fs::OpenOptions::new().write(true).create_new(true).open(&destination).map_err(|e| e.to_string())?;
            output.write_all(&candidate.bytes).map_err(|e| e.to_string())?;
            output.sync_all().map_err(|e| e.to_string())?;
        }
        let installed = RegisteredStyle { path: destination.to_string_lossy().into_owned(), ..candidate };
        if !StyleRegistry::unchanged(&installed) {
            return Err(l.get("guidedPanel.approvalRequired"));
        }
        trust(&writer).decide(&installed, "approved").map_err(|e| e.0)
    })
    .await
    .map_err(|e| e.to_string())??;
    reload_all(host);
    Ok(())
}

/// C#'s allow, revoke and unblock buttons: allow opens the approval sheet; the others decide at once.
pub async fn decide(host: &Arc<Host>, style: RegisteredStyle, action: &str) -> Result<(), String> {
    if !host.writes() {
        host.log.line("styles: a decision is not stored (no --allow-state-write)");
        return Ok(());
    }
    if action == "approved" {
        approve(host, &style).await?;
    } else {
        let (writer, action) = (host.clone(), action.to_string());
        tauri::async_runtime::spawn_blocking(move || trust(&writer).decide(&style, &action).map_err(|e| e.0))
            .await
            .map_err(|e| e.to_string())??;
    }
    reload_all(host);
    Ok(())
}

/// C#'s remove button: confirmed (D12), the file checked unchanged and inside `<data>\styles`, its decision forgotten, then deleted.
pub async fn remove(host: &Arc<Host>, style: RegisteredStyle) -> Result<(), String> {
    if !host.writes() {
        host.log.line("styles: a removal is not done (no --allow-state-write)");
        return Ok(());
    }
    if !confirm_remove(host, &style.path).await {
        return Ok(());
    }
    let writer = host.clone();
    tauri::async_runtime::spawn_blocking(move || -> Result<(), String> {
        let l = writer.locale;
        if !StyleRegistry::unchanged(&style) {
            return Err(l.get("guidedPanel.approvalRequired"));
        }
        let root = mighty_core::workspace_files::real_path(&writer.data_dir.join("styles"));
        if !root.is_some_and(|root| mighty_core::workspace_files::contains(Path::new(&style.path), &root)) {
            return Err(l.get("guidedPanel.approvalRequired"));
        }
        // The decision goes with the file: the same style registered again is asked about afresh.
        trust(&writer).forget(&style).map_err(|e| e.0)?;
        std::fs::remove_file(&style.path).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())??;
    reload_all(host);
    Ok(())
}

/// Every id the panes keep a style for (the smoke's report).
pub fn loaded(id: &str) -> (bool, Option<String>, usize) {
    let all = panes();
    let entry = all.get(id);
    (
        entry.and_then(|e| e.loaded_key.as_ref()).is_some(),
        entry.and_then(|e| e.active.as_ref()).map(|s| s.id().to_string()),
        entry.and_then(|e| e.registry.as_ref()).map_or(0, |r| r.styles.len()),
    )
}
