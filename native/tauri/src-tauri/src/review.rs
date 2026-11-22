//! 검토 대기열: one list of everything ready to review — the worktree panes and delegated children that finished with work in their
//! worktree, and each pane's newest finished request whose changes the originals store captured (요청 변경 검토). The page draws it
//! (`src/review/ReviewPanel.tsx`, opened from the sidebar's 검토 entry and ⇧⌘R / Ctrl+Shift+R) and runs its actions through the methods
//! that already do them: `paneWorktreeAction` (변경 보기, 병합, 커밋하고 병합, 버리기, PR 열기, with their confirmations), `childAction`
//! (병합, 버리기), `openInEditor`, and the page's own 요청 변경 검토 panel. Here: `reviewQueue{refresh?}` answers the model,
//! `reviewChanges{id}` shows a child's changes sheet, `reviewDismiss{key}` hides an item until its work changes.
//!
//! Cheap by design: git is read only when the queue is refreshed — when a run ends (debounced, [`changed`]), when the panel opens or
//! its refresh button is pressed, and after one of its actions — never per frame. Each refresh reads a worktree twice-and-a-bit (its
//! status, `diff --numstat`, its untracked files) and the originals store once per pane; the model is kept and pushed as `{kind:
//! 'review', queue}` only when it changed.

use std::collections::HashSet;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Duration;

use mighty_core::model::PaneWorktree;
use mighty_core::shell::ChildState;
use mighty_delegation::cards::DelegationCardResult;
use mighty_delegation::pane_worktree::{DiffStat, Status};
use serde_json::{Value, json};

use crate::host::{Host, Registry, lock};
use crate::methods::text;

/// How long after a run ends the queue is read again (several runs ending together read it once).
const SETTLE: Duration = Duration::from_millis(1500);

/// The terminal requests a pane's newest-first look goes through before it gives up on finding captured changes.
const REQUESTS_LOOKED_AT: usize = 6;

#[derive(Default)]
struct Queue {
    /// The model last built, as JSON text (what was pushed).
    model: Option<Value>,
    /// The items hidden until their work changes (their keys carry the counts).
    dismissed: HashSet<String>,
    /// A refresh waits on its timer.
    scheduled: bool,
}

static QUEUE: LazyLock<Mutex<Queue>> = LazyLock::new(Mutex::default);

/// Which worktree an item is (a request is an item of its own, [`RequestSeed`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Worktree,
    Child,
}

impl Kind {
    fn word(self) -> &'static str {
        match self {
            Kind::Worktree => "worktree",
            Kind::Child => "child",
        }
    }
}

/// An item's state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Shown {
    Conflict,
    Ready,
    Merged,
}

impl Shown {
    fn word(self) -> &'static str {
        match self {
            Shown::Conflict => "conflict",
            Shown::Ready => "ready",
            Shown::Merged => "merged",
        }
    }
}

/// One worktree as the queue reads it: a worktree pane's or a delegated child's.
#[derive(Debug, Clone, PartialEq)]
pub struct WorktreeSeed {
    pub kind: Kind,
    /// The pane's session id (a child's record id is its pane's).
    pub id: String,
    pub title: String,
    pub agent: String,
    pub workspace: String,
    /// The workspace's id (a child's is its parent pane's): the Git workbench's 점검 tab opens on it.
    pub workspace_id: String,
    pub branch: String,
    pub base: String,
    /// Its agent still works (a pane's run, a child that is creating, running or waiting): not ready yet.
    pub working: bool,
    /// The worktree's branch went into its base and nothing came after (a pane's `merged_commit`; a child's `merged` state).
    pub merged: bool,
    pub conflict: bool,
    pub busy: bool,
    /// What git said; None when it could not (the worktree is gone).
    pub reading: Option<(Status, DiffStat)>,
    /// 머지 전 점검's warnings (merge_checks.rs `queue_warnings`); None when not read (its agent works); a merged item carries none.
    pub warnings: Option<Vec<crate::merge_checks::Warning>>,
}

/// One pane's newest finished request with captured changes.
#[derive(Debug, Clone, PartialEq)]
pub struct RequestSeed {
    pub pane_id: String,
    pub run_id: String,
    pub title: String,
    pub request: String,
    pub agent: String,
    pub workspace: String,
    /// Files, lines the request added, lines it removed, and the files 검토됨 (바뀐 부분별 받기·버리기: every hunk taken or put back).
    pub counts: crate::files::RequestCounts,
}

/// Whether a worktree is listed and as what: never while its agent works or with nothing to review; a conflict before anything; work
/// left (commits ahead or changed files) is ready; a merged worktree with nothing more stays listed as merged until it is discarded.
pub fn worktree_state(seed: &WorktreeSeed) -> Option<Shown> {
    if seed.working {
        return None;
    }
    let work = seed.reading.is_some_and(|(status, stat)| status.ahead > 0 || stat.files > 0);
    if seed.conflict && seed.reading.is_some() {
        return Some(Shown::Conflict);
    }
    if work && !(seed.kind == Kind::Child && seed.merged) {
        return Some(Shown::Ready);
    }
    seed.merged.then_some(Shown::Merged)
}

/// The key an item is dismissed by: its kind, id and what its work is now, so new work shows it again.
fn worktree_key(seed: &WorktreeSeed, shown: Shown) -> String {
    let (ahead, stat) = seed.reading.map_or((0, DiffStat::default()), |(status, stat)| (status.ahead, stat));
    format!("{}:{}:{}:{ahead}:{}:{}:{}", seed.kind.word(), seed.id, shown.word(), stat.files, stat.added, stat.removed)
}

/// The queue's model: `{items, count}`, conflicts first, then the ready ones, then the merged ones (each group in the order given);
/// `count` is what is not merged. Dismissed items are left out, and so is a request whose every file is 검토됨.
pub fn model(worktrees: &[WorktreeSeed], requests: &[RequestSeed], dismissed: &HashSet<String>) -> Value {
    let mut items: Vec<(Shown, Value)> = vec![];
    for seed in worktrees {
        let Some(shown) = worktree_state(seed) else { continue };
        let key = worktree_key(seed, shown);
        if dismissed.contains(&key) {
            continue;
        }
        let (status, stat) = seed.reading.unwrap_or((Status { ahead: 0, behind: 0, changed_files: 0 }, DiffStat::default()));
        items.push((
            shown,
            json!({
                "key": key, "kind": seed.kind.word(), "id": seed.id, "paneId": seed.id, "title": seed.title, "agent": seed.agent,
                "workspace": seed.workspace, "branch": seed.branch, "base": seed.base, "status": shown.word(), "busy": seed.busy,
                "files": stat.files, "added": stat.added, "removed": stat.removed, "ahead": status.ahead, "behind": status.behind,
                "uncommitted": status.changed_files, "canMerge": shown != Shown::Merged,
                "canCommitMerge": seed.kind == Kind::Worktree && shown != Shown::Merged && status.changed_files > 0,
                "canPr": seed.kind == Kind::Worktree && shown != Shown::Merged, "workspaceId": seed.workspace_id,
                "warnings": if shown == Shown::Merged { None } else { seed.warnings.clone() },
            }),
        ));
    }
    for seed in requests {
        let key = format!("request:{}:{}", seed.pane_id, seed.run_id);
        if dismissed.contains(&key) {
            continue;
        }
        let crate::files::RequestCounts { files, added, removed, reviewed } = seed.counts;
        if reviewed >= files {
            continue;
        }
        items.push((
            Shown::Ready,
            json!({
                "key": key, "kind": "request", "id": seed.pane_id, "paneId": seed.pane_id, "runId": seed.run_id, "title": seed.title,
                "request": seed.request, "agent": seed.agent, "workspace": seed.workspace, "status": "ready", "busy": false,
                "files": files, "added": added, "removed": removed, "reviewed": reviewed, "canMerge": false, "canCommitMerge": false,
                "canPr": false,
            }),
        ));
    }
    items.sort_by_key(|(shown, _)| *shown);
    let count = items.iter().filter(|(shown, _)| *shown != Shown::Merged).count();
    json!({"items": items.into_iter().map(|(_, item)| item).collect::<Vec<_>>(), "count": count})
}

/// What the snapshot says of the panes, read under its lock: the worktree panes (with their records) and, for the others, the run
/// folder and the newest finished requests to look at.
struct Seeds {
    panes: Vec<(WorktreeSeed, PaneWorktree)>,
    requests: Vec<(RequestSeed, String, Vec<String>)>,
}

fn seeds(host: &Host) -> Seeds {
    let view = host.view();
    let snapshot = &view.snapshot;
    let mut panes = vec![];
    let mut requests = vec![];
    for session in snapshot.sessions() {
        let Some(workspace) = snapshot.workspace(session.workspace_id()).filter(|w| w.remote.is_none()) else { continue };
        let agent = mighty_composer::runtime::provider_label(session.provider()).to_string();
        let title = if session.title().is_empty() { agent.clone() } else { session.title().to_string() };
        if let Some(record) = session.pane_worktree.clone() {
            let seed = WorktreeSeed {
                kind: Kind::Worktree,
                id: session.id().to_string(),
                title,
                agent,
                workspace: workspace.name().to_string(),
                workspace_id: workspace.id().to_string(),
                branch: record.branch().to_string(),
                base: record.base_branch().to_string(),
                working: session.status() == "running",
                merged: record.merged_commit.is_some(),
                conflict: false,
                busy: false,
                reading: None,
                warnings: None,
            };
            panes.push((seed, record));
            continue;
        }
        // A delegated child's work is its worktree's (listed below); a shell or a files pane makes no requests.
        if session.parent_session_id.is_some() || session.graph_runs().is_empty() {
            continue;
        }
        let candidates: Vec<String> = session
            .graph_runs()
            .iter()
            .rev()
            .filter(|run| mighty_core::graph::terminal(run.status.as_deref().unwrap_or("")))
            .take(REQUESTS_LOOKED_AT)
            .map(|run| run.source_run_id.clone().unwrap_or_else(|| run.id().to_string()))
            .filter(|id| !id.is_empty())
            .collect();
        if candidates.is_empty() {
            continue;
        }
        let root = session.working_folder.clone().unwrap_or_else(|| workspace.path().to_string());
        let seed = RequestSeed {
            pane_id: session.id().to_string(),
            run_id: String::new(),
            title,
            request: String::new(),
            agent,
            workspace: workspace.name().to_string(),
            counts: crate::files::RequestCounts::default(),
        };
        requests.push((seed, root, candidates));
    }
    Seeds { panes, requests }
}

/// The request a graph run `id` of the pane asked (shortened to one line), as the snapshot has it now.
fn request_text(host: &Host, pane: &str, run: &str) -> (String, String) {
    let view = host.view();
    let found = view.snapshot.session(pane).and_then(|session| {
        session
            .graph_runs()
            .iter()
            .rev()
            .find(|r| r.source_run_id.as_deref().unwrap_or(r.id()) == run)
            .map(|r| (r.id().to_string(), r.input.as_deref().and_then(mighty_core::normalize::shortened).unwrap_or_default()))
    });
    found.unwrap_or_else(|| (run.to_string(), String::new()))
}

/// Reads everything again and keeps the model; pushes it when it changed.
pub async fn refresh(host: &Arc<Host>) -> Value {
    let Seeds { panes, requests } = seeds(host);
    let mut worktrees = vec![];
    for (mut seed, record) in panes {
        if !seed.working {
            seed.reading = crate::pane_worktree::review_reading(&record).await;
            seed.warnings = seed.reading.map(|(status, _)| {
                crate::merge_checks::queue_warnings(host, &seed.id, &record, usize::try_from(status.changed_files).unwrap_or(0))
            });
        }
        (seed.conflict, seed.busy) = crate::pane_worktree::review_flags(&seed.id);
        worktrees.push(seed);
    }
    if let Some(app) = crate::delegation::app() {
        for row in app.rows() {
            let working = matches!(row.state, ChildState::Creating | ChildState::Running | ChildState::Waiting);
            let Some(record) = crate::pane_worktree::child_record(&row.id) else { continue };
            let open = host.view().snapshot.session(&row.id).map(|session| session.title().to_string()).filter(|t| !t.is_empty());
            let agent = mighty_composer::runtime::provider_label(&row.agent).to_string();
            let conflict = app
                .card_note(&row.id)
                .is_some_and(|note| matches!(note.result, DelegationCardResult::ConflictSent | DelegationCardResult::ConflictNotStarted));
            let mut seed = WorktreeSeed {
                kind: Kind::Child,
                id: row.id.clone(),
                title: open.or_else(|| row.task.clone()).unwrap_or_else(|| agent.clone()),
                agent,
                workspace: String::new(),
                workspace_id: String::new(),
                branch: record.branch().to_string(),
                base: record.base_branch().to_string(),
                working,
                merged: row.state == ChildState::Merged,
                conflict,
                busy: app.is_card_busy(&row.id),
                reading: None,
                warnings: None,
            };
            if !working && row.state != ChildState::Merged {
                seed.reading = crate::pane_worktree::review_reading(&record).await;
                seed.warnings = seed.reading.map(|(status, _)| {
                    crate::merge_checks::queue_warnings(host, &row.id, &record, usize::try_from(status.changed_files).unwrap_or(0))
                });
            }
            // The child's workspace is its parent pane's (read under one lock).
            (seed.workspace, seed.workspace_id) = {
                let view = host.view();
                let parent = view.snapshot.session(&row.parent_session_id);
                parent
                    .and_then(|p| view.snapshot.workspace(p.workspace_id()))
                    .map(|w| (w.name().to_string(), w.id().to_string()))
                    .unwrap_or_default()
            };
            worktrees.push(seed);
        }
    }
    let mut found = vec![];
    for (mut seed, root, candidates) in requests {
        let reader = host.clone();
        let pane = seed.pane_id.clone();
        let changed = tauri::async_runtime::spawn_blocking(move || {
            candidates.iter().find_map(|run| crate::files::request_changes(&reader, &pane, run, &root).map(|counts| (run.clone(), counts)))
        })
        .await
        .ok()
        .flatten();
        let Some((run, counts)) = changed else { continue };
        let (run_id, request) = request_text(host, &seed.pane_id, &run);
        seed.run_id = run_id;
        seed.request = request;
        seed.counts = counts;
        found.push(seed);
    }
    let dismissed = lock(&QUEUE).dismissed.clone();
    let built = model(&worktrees, &found, &dismissed);
    let changed = {
        let mut queue = lock(&QUEUE);
        let changed = queue.model.as_ref() != Some(&built);
        queue.model = Some(built.clone());
        changed
    };
    if changed {
        let view = host.view();
        host.push(&view, "review", json!({"kind": "review", "queue": built}).to_string());
    }
    built
}

/// A run ended (host.rs) or a child's state moved: the queue is read again once things settle.
pub fn changed(host: &Arc<Host>) {
    {
        let mut queue = lock(&QUEUE);
        if queue.scheduled {
            return;
        }
        queue.scheduled = true;
    }
    let host = host.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(SETTLE).await;
        lock(&QUEUE).scheduled = false;
        if !host.closing() {
            refresh(&host).await;
        }
    });
}

pub fn register(methods: &mut Registry) {
    // The model: the one kept unless `refresh` asks for a new reading (the first call always reads).
    methods.add("reviewQueue", |host: Arc<Host>, args| async move {
        let kept = lock(&QUEUE).model.clone();
        match kept {
            Some(model) if !crate::methods::flag(&args, "refresh") => Ok(model),
            _ => Ok(refresh(&host).await),
        }
    });
    methods.add("reviewChanges", |host: Arc<Host>, args| async move {
        if let Some(id) = text(&args, "id") {
            crate::pane_worktree::show_child_changes(&host, id).await;
        }
        Ok(Value::Null)
    });
    methods.add("reviewDismiss", |host: Arc<Host>, args| async move {
        if let Some(key) = text(&args, "key").filter(|key| key.len() <= 1024) {
            lock(&QUEUE).dismissed.insert(key.to_string());
        }
        Ok(refresh(&host).await)
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn worktree(kind: Kind, id: &str, reading: Option<(i32, usize)>) -> WorktreeSeed {
        WorktreeSeed {
            kind,
            id: id.into(),
            title: format!("pane {id}"),
            agent: "Claude".into(),
            workspace: "app".into(),
            workspace_id: "ws".into(),
            branch: format!("mighty/{id}"),
            base: "main".into(),
            working: false,
            merged: false,
            conflict: false,
            busy: false,
            reading: reading.map(|(ahead, files)| {
                (Status { ahead, behind: 0, changed_files: 1 }, DiffStat { files, added: files * 3, removed: files })
            }),
            warnings: None,
        }
    }

    #[test]
    fn a_worktree_is_listed_once_its_agent_stops_and_only_with_work() {
        let ready = worktree(Kind::Worktree, "a", Some((2, 3)));
        assert_eq!(worktree_state(&ready), Some(Shown::Ready));
        assert_eq!(worktree_state(&WorktreeSeed { working: true, ..ready.clone() }), None, "its agent still works");
        assert_eq!(worktree_state(&worktree(Kind::Worktree, "b", Some((0, 0)))), None, "nothing to review");
        assert_eq!(worktree_state(&worktree(Kind::Worktree, "c", None)), None, "git could not say");
        assert_eq!(worktree_state(&WorktreeSeed { conflict: true, ..ready.clone() }), Some(Shown::Conflict));
        let merged = WorktreeSeed { merged: true, ..worktree(Kind::Worktree, "d", Some((0, 0))) };
        assert_eq!(worktree_state(&merged), Some(Shown::Merged));
        assert_eq!(worktree_state(&WorktreeSeed { merged: true, ..ready }), Some(Shown::Ready), "a pane with work after its merge");
        let child = WorktreeSeed { merged: true, ..worktree(Kind::Child, "e", None) };
        assert_eq!(worktree_state(&child), Some(Shown::Merged), "a merged child, its worktree read or not");
    }

    #[test]
    fn the_model_puts_conflicts_first_counts_what_is_not_merged_and_leaves_out_what_was_dismissed() {
        let warned = vec![crate::merge_checks::Warning { code: "checksFailed", count: 1 }];
        let worktrees = vec![
            WorktreeSeed { merged: true, warnings: Some(warned.clone()), ..worktree(Kind::Worktree, "m", Some((0, 0))) },
            WorktreeSeed { warnings: Some(warned), ..worktree(Kind::Worktree, "r", Some((1, 2))) },
            WorktreeSeed { conflict: true, ..worktree(Kind::Child, "c", Some((1, 1))) },
            WorktreeSeed { working: true, ..worktree(Kind::Child, "w", Some((1, 1))) },
        ];
        let requests = vec![RequestSeed {
            pane_id: "p".into(),
            run_id: "run-1".into(),
            title: "pane p".into(),
            request: "fix the bug".into(),
            agent: "Codex".into(),
            workspace: "app".into(),
            counts: crate::files::RequestCounts { files: 2, added: 10, removed: 4, reviewed: 1 },
        }];
        let built = model(&worktrees, &requests, &HashSet::new());
        let items = built["items"].as_array().unwrap();
        let order: Vec<(&str, &str)> = items.iter().map(|i| (i["id"].as_str().unwrap(), i["status"].as_str().unwrap())).collect();
        assert_eq!(order, [("c", "conflict"), ("r", "ready"), ("p", "ready"), ("m", "merged")]);
        assert_eq!(built["count"], 3);
        let ready = &items[1];
        assert_eq!((ready["files"].as_u64(), ready["added"].as_u64(), ready["removed"].as_u64()), (Some(2), Some(6), Some(2)));
        assert_eq!((ready["kind"].as_str(), ready["branch"].as_str()), (Some("worktree"), Some("mighty/r")));
        assert_eq!((ready["canCommitMerge"].as_bool(), ready["canPr"].as_bool()), (Some(true), Some(true)));
        assert_eq!(
            (items[0]["canCommitMerge"].as_bool(), items[0]["canPr"].as_bool()),
            (Some(false), Some(false)),
            "a child merges by its card"
        );
        assert_eq!((items[2]["runId"].as_str(), items[2]["canMerge"].as_bool()), (Some("run-1"), Some(false)));
        assert_eq!(items[3]["canMerge"].as_bool(), Some(false));
        assert_eq!(ready["warnings"][0]["code"], "checksFailed", "the pre-merge checks summary");
        assert_eq!(ready["workspaceId"], "ws");
        assert!(items[3]["warnings"].is_null(), "a merged item carries none");
        assert!(items[0]["warnings"].is_null(), "not read");

        let dismissed: HashSet<String> = [ready["key"].as_str().unwrap().to_string(), "request:p:run-1".to_string()].into();
        let again = model(&worktrees, &requests, &dismissed);
        assert_eq!(again["count"], 1);
        // New work in the dismissed worktree shows it again: its key carries the counts.
        let mut more = worktrees.clone();
        more[1] = worktree(Kind::Worktree, "r", Some((2, 5)));
        assert_eq!(model(&more, &requests, &dismissed)["count"], 2);
    }

    /// 바뀐 부분별 받기·버리기: a request counts until every file it changed is 검토됨; the item says how many are.
    #[test]
    fn a_request_whose_files_are_all_reviewed_leaves_the_queue() {
        let request = |reviewed| RequestSeed {
            pane_id: "p".into(),
            run_id: "run-1".into(),
            title: "pane p".into(),
            request: "fix".into(),
            agent: "Claude".into(),
            workspace: "app".into(),
            counts: crate::files::RequestCounts { files: 3, added: 5, removed: 1, reviewed },
        };
        let partly = model(&[], &[request(2)], &HashSet::new());
        assert_eq!(partly["count"], 1);
        assert_eq!(partly["items"][0]["reviewed"], 2);
        let done = model(&[], &[request(3)], &HashSet::new());
        assert_eq!(done["count"], 0);
        assert!(done["items"].as_array().unwrap().is_empty());
    }
}
