//! The coordinator (C# Core/DelegationCoordinator.cs; M/DelegationCoordinator.swift): the manifest, the gates in order, `ArgumentProblem`,
//! `LateMessage` after 45 s, the FIFO turns for merges and run events, and the seams W7-D and W7-C call (`Coordinator::{new, handle,
//! start_human_child, close_child, discard_child, merge_from_card, undo_merge_from_card, child_run_started, child_run_ended, deliver_pending, send,
//! run_next, held_items_by_pane, rows, interrupt_runs_killed_by_quit}`; it implements [`DelegationHandler`]). Owner: W7-A.
//!
//! Concurrency (W7 plan section 1.9). C# runs the coordinator on an exclusive scheduler: one continuation at a time, interleaving only
//! at awaits, and everything re-reads the file after every await ("nothing below suspends until saved"). Here the state sits behind a
//! `std::sync::Mutex` held only across those synchronous re-read, check, mutate and save sections — never across an await (the crate
//! denies `clippy::await_holding_lock`). Merges and run events take FIFO turns (tokio mutexes, held across awaits by design). The host
//! is called on a task of its own, never with the state locked; a host call that panics is a call the host did not take.
//!
//! The coordinator's parts live beside their C# files: [`crate::delivery`] (`DelegationDelivery.cs`), [`crate::reports`]
//! (`DelegationReports.cs`), [`crate::tools`] (the six tools), [`crate::cards`] (`DelegationCards.cs` and the close and discard of
//! `DelegationCleanup.cs`).

use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use indexmap::IndexMap;
use mighty_core::locale::Locale;
use mighty_core::shell::DelegationChildRow;
use mighty_platform::BoxFuture;
use tokio::sync::watch;

use crate::delivery::DelegationDeliveryItem;
use crate::git::paths::DelegationPaths;
use crate::git::worktree::Maker;
use crate::host::{DelegationHost, DelegationPaneState};
use crate::model::{ChildState, DeliveryLane, DeliveryRoute};
use crate::pruning::DelegationPruneContext;
use crate::request::{
    DelegationCaller, DelegationHandler, DelegationReasonCode, DelegationRequest, DelegationResponse, unknown_tool_message,
};
use crate::sidebar;
use crate::store::{DelegationFile, DelegationFileError, DelegationFileStore, SaveError};
use crate::tools::{self, Tool};

/// `DelegationCoordinator.AnswerSeconds`: the longest any call takes to answer, well inside the limit of a tool call's transport.
pub const ANSWER_SECONDS: f64 = 45.0;
/// `MaximumOpenChildren`: the open children one parent may have at once (of each kind: its delegate's, and the human's).
pub const MAXIMUM_OPEN_CHILDREN: usize = 3;
/// `MaximumDepth`: a parent's children cannot delegate.
pub const MAXIMUM_DEPTH: usize = 1;
/// `MaximumFollowUps`: the follow-ups one child may get from its parent.
pub const MAXIMUM_FOLLOW_UPS: i32 = 2;
/// `MaximumFollowUpBytes`: the most a follow-up's text may take in the delegation file, its JSON escapes included — 48 KiB, so a child's
/// two follow-ups released together always fit one run's input.
pub const MAXIMUM_FOLLOW_UP_BYTES: usize = 48 * 1024;
/// `NoAnswerMessage`.
pub const NO_ANSWER_MESSAGE: &str = "Mighty Code did not answer the request.";

/// A run the app's send starts with the run's whole input (the held items and the human's text): the new run's id, or None.
pub type RunStarter = Box<dyn FnOnce(String) -> BoxFuture<'static, Option<String>> + Send>;

/// `LateMessage`.
pub fn late_message(tool: &str, seconds: f64) -> String {
    format!(
        "{tool} did not finish within {} seconds. Its work goes on in Mighty Code; call list_children to see where your children stand.",
        seconds as i64
    )
}

/// `NotYourChildMessage`.
pub fn not_your_child_message(id: &str) -> String {
    format!("No child \"{}\" of yours. list_children names your children.", crate::request::prefix(id, 80))
}

/// `ArgumentProblem`: why `arguments` are not `tool`'s (every required one, and no other), or None when they are.
pub fn argument_problem<'a>(arguments: impl IntoIterator<Item = &'a String> + Clone, tool: &Tool) -> Option<String> {
    let names: Vec<&str> = tool.arguments.iter().map(|argument| argument.name).collect();
    let required: Vec<&str> = tool.arguments.iter().filter(|argument| argument.required).map(|argument| argument.name).collect();
    let given: Vec<&String> = arguments.into_iter().collect();
    if given.iter().all(|name| names.contains(&name.as_str())) && required.iter().all(|name| given.iter().any(|given| given == name)) {
        return None;
    }
    if names.is_empty() {
        return Some(format!("{} takes no arguments.", tool.name));
    }
    let optional: Vec<&str> = tool.arguments.iter().filter(|argument| !argument.required).map(|argument| argument.name).collect();
    Some(format!(
        "{} takes exactly these string arguments: {}{}",
        tool.name,
        required.join(", "),
        if optional.is_empty() { ".".to_string() } else { format!(", and optionally {}.", optional.join(", ")) }
    ))
}

/// `Answer`: `work`'s answer, or `late` once `seconds` pass first. The work is not cancelled: it finishes in the background and keeps
/// what it records, but its answer is dropped, so a call is answered exactly once. Work that panicked is answered as a call that
/// reached no answer.
pub async fn answer(
    seconds: f64,
    late: DelegationResponse,
    work: impl Future<Output = DelegationResponse> + Send + 'static,
) -> DelegationResponse {
    let mut task = tokio::spawn(work);
    match tokio::time::timeout(Duration::from_secs_f64(seconds.max(0.0)), &mut task).await {
        Err(_) => late,
        Ok(Ok(response)) => response,
        Ok(Err(_)) => DelegationResponse::failure(NO_ANSWER_MESSAGE),
    }
}

/// Runs a host call on a task of its own: its answer, or None when it panicked (C#: "a host that throws is a host that did not take the
/// call").
pub(crate) async fn host_task<T: Send + 'static>(call: impl Future<Output = T> + Send + 'static) -> Option<T> {
    tokio::spawn(call).await.ok()
}

/// `ChildRunsEnding`: the children whose run end is reported but not yet recorded. The host marks one as it reports the end, and the
/// coordinator clears it once the end is recorded; a child reported twice before the first is recorded stays marked until both are.
/// Until then delivery hands the child nothing and the app starts no run in its pane.
#[derive(Debug, Default)]
pub struct ChildRunsEnding {
    counts: Mutex<HashMap<String, usize>>,
}

impl ChildRunsEnding {
    fn counts(&self) -> MutexGuard<'_, HashMap<String, usize>> {
        self.counts.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// `Begin`: the host is about to report an end of the child `id`'s run.
    pub fn begin(&self, id: &str) {
        *self.counts().entry(id.to_string()).or_default() += 1;
    }

    pub fn contains(&self, id: &str) -> bool {
        self.counts().contains_key(id)
    }

    /// `Finish`: one reported end of the child `id` is recorded; false when none was marked.
    pub(crate) fn finish(&self, id: &str) -> bool {
        let mut counts = self.counts();
        match counts.get_mut(id) {
            None => false,
            Some(count) if *count > 1 => {
                *count -= 1;
                true
            }
            Some(_) => {
                counts.remove(id);
                true
            }
        }
    }
}

/// What the coordinator needs besides its file and its host (C#'s constructor arguments).
pub struct CoordinatorOptions {
    /// Makes each child's worktree: under `%USERPROFILE%\.mightycode\worktrees` in the app, under a temp folder in tests.
    pub worktrees: Maker,
    /// The OS's final names of paths (the discard's nested-worktree comparison, cleanup, discard).
    pub paths: DelegationPaths,
    /// The switch (`DelegationSwitch.IsOn`), read at each call.
    pub switch_on: Arc<dyn Fn() -> bool + Send + Sync>,
    /// At most [`ANSWER_SECONDS`].
    pub answer_seconds: f64,
    /// The app's language now (C# reads `Locale.LanguagePreference` at each call).
    pub locale: Arc<dyn Fn() -> Locale + Send + Sync>,
}

/// What the coordinator holds that changes, read and changed only under its lock.
pub(crate) struct State {
    pub(crate) file: DelegationFile,
    /// The report revision at which each child was last asked to resolve its conflict with the parent branch.
    pub(crate) conflict_asked: HashMap<String, i32>,
    /// Children whose start is still going: delegate saved their record and their start has not finished. A discard waits for it.
    pub(crate) starting: HashSet<String>,
    /// The panes whose checkout a card's merge or undo is changing: delivery starts no run in them until it is over.
    pub(crate) card_panes: HashSet<String>,
    /// Whether a delivery pass is offering pending items, and whether another pass was asked for meanwhile.
    pub(crate) is_delivering: bool,
    pub(crate) wants_delivery: bool,
}

pub(crate) struct Inner {
    pub(crate) store: DelegationFileStore,
    pub(crate) host: Arc<dyn DelegationHost>,
    pub(crate) worktrees: Maker,
    pub(crate) paths: DelegationPaths,
    pub(crate) switch_on: Arc<dyn Fn() -> bool + Send + Sync>,
    pub(crate) answer_seconds: f64,
    pub(crate) locale: Arc<dyn Fn() -> Locale + Send + Sync>,
    pub(crate) state: Mutex<State>,
    pub(crate) ending_runs: ChildRunsEnding,
    /// A merge (the tool's or a card's) or a card's undo or discard: they take turns, so two never run git in a parent's checkout at
    /// once.
    pub(crate) merge_turns: tokio::sync::Mutex<()>,
    /// A run event of a child's pane (its start, its end, the parent awaiting it, or the launch's sweep): each is recorded whole before
    /// the next.
    pub(crate) run_turns: tokio::sync::Mutex<()>,
    rows: watch::Sender<Vec<DelegationChildRow>>,
    held_rows: watch::Sender<IndexMap<String, Vec<DelegationDeliveryItem>>>,
}

/// `DelegationCoordinator`: the app's delegation service. It owns the child records (the delegation file is their authority), the six
/// tools, delivery and the git actions, and reaches panes only through its [`DelegationHost`]. Each call is answered by the first rule
/// that applies:
/// 1. the gates ([`crate::providers::gate`]): switched off or a run without the server reaches nothing; a pane whose agent has no
///    delegation tools is refused with claude_only;
/// 2. a name that is not one of the six tools reaches nothing;
/// 3. discard is always refused with discard_human_only;
/// 4. a call without exactly the tool's string arguments is answered with an error and changes nothing;
/// 5. otherwise the tool runs, and its answer comes within the answer time.
///
/// A cheap handle: clones share one coordinator.
#[derive(Clone)]
pub struct Coordinator {
    pub(crate) inner: Arc<Inner>,
}

impl std::fmt::Debug for Coordinator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Coordinator").field("store", &self.inner.store).finish_non_exhaustive()
    }
}

impl Coordinator {
    /// Loads the delegation file from `store`; Unreadable when it cannot be read (delegation is then off for the launch). What the last
    /// launch left pending is held, never delivered on its own, and saved so when the disk lets it (the move only shortens the file).
    pub fn new(
        store: DelegationFileStore,
        host: Arc<dyn DelegationHost>,
        options: CoordinatorOptions,
    ) -> Result<Coordinator, DelegationFileError> {
        let mut file = store.load()?;
        if let Some(held) = file.holding_pending() {
            file = held;
            let _ = store.save(&file);
        }
        let (rows, _) = watch::channel(sidebar::rows(&file));
        let (held_rows, _) = watch::channel(file.held_items_by_pane());
        Ok(Coordinator {
            inner: Arc::new(Inner {
                store,
                host,
                worktrees: options.worktrees,
                paths: options.paths,
                switch_on: options.switch_on,
                answer_seconds: options.answer_seconds.min(ANSWER_SECONDS),
                locale: options.locale,
                state: Mutex::new(State {
                    file,
                    conflict_asked: HashMap::new(),
                    starting: HashSet::new(),
                    card_panes: HashSet::new(),
                    is_delivering: false,
                    wants_delivery: false,
                }),
                ending_runs: ChildRunsEnding::default(),
                merge_turns: tokio::sync::Mutex::new(()),
                run_turns: tokio::sync::Mutex::new(()),
                rows,
                held_rows,
            }),
        })
    }

    pub fn store(&self) -> &DelegationFileStore {
        &self.inner.store
    }

    /// `Worktrees`: the maker (the composer asks it whether a folder can host a child worktree).
    pub fn worktrees(&self) -> &Maker {
        &self.inner.worktrees
    }

    /// `EndingRuns`.
    pub fn ending_runs(&self) -> &ChildRunsEnding {
        &self.inner.ending_runs
    }

    /// `File`: the delegation file as last loaded or saved — or a start, merge or undo the disk refused to save, held until the next
    /// save writes it.
    pub fn file(&self) -> DelegationFile {
        self.lock().file.clone()
    }

    /// The children as the sidebar lists them now ([`sidebar::rows`]).
    pub fn rows(&self) -> Vec<DelegationChildRow> {
        sidebar::rows(&self.lock().file)
    }

    /// `ChildRows`: the rows at load, then after each change to them; only the newest waits.
    pub fn child_rows(&self) -> watch::Receiver<Vec<DelegationChildRow>> {
        self.inner.rows.subscribe()
    }

    /// `HeldRows`: the items held for each pane, oldest first, at load and after each change; only the newest waits.
    pub fn held_rows(&self) -> watch::Receiver<IndexMap<String, Vec<DelegationDeliveryItem>>> {
        self.inner.held_rows.subscribe()
    }

    /// `PendingWork`: delegation work a restart would cut short — notices and follow-ups waiting to be offered, children still starting,
    /// the panes a card's merge or undo is changing, and a merge, undo or discard going. The automatic app update waits while it is
    /// above zero.
    pub fn pending_work(&self) -> usize {
        let state = self.lock();
        let pending = state.file.notices.iter().filter(|n| n.lane == DeliveryLane::Pending).count()
            + state.file.follow_ups.iter().filter(|f| f.lane == DeliveryLane::Pending).count();
        pending + state.starting.len() + state.card_panes.len() + usize::from(self.inner.merge_turns.try_lock().is_err())
    }

    /// `IsStarting`: the child's start is still going (delegate answered; its worktree, pane and first run are not all settled yet).
    pub fn is_starting(&self, id: &str) -> bool {
        self.lock().starting.contains(id)
    }

    pub(crate) fn lock(&self) -> MutexGuard<'_, State> {
        self.inner.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    pub(crate) fn locale(&self) -> Locale {
        (self.inner.locale)()
    }

    /// The file changed: the published rows follow when they differ.
    fn set_file(&self, state: &mut State, file: DelegationFile) {
        state.file = file;
        let rows = sidebar::rows(&state.file);
        self.inner.rows.send_if_modified(|published| {
            let changed = *published != rows;
            if changed {
                *published = rows;
            }
            changed
        });
        let held = state.file.held_items_by_pane();
        self.inner.held_rows.send_if_modified(|published| {
            let changed = *published != held;
            if changed {
                *published = held;
            }
            changed
        });
    }

    /// `File = next` without a save (a change the disk refused, held until the next save).
    pub(crate) fn keep(&self, state: &mut State, next: DelegationFile) {
        self.set_file(state, next);
    }

    /// `Commit`: saves `next`, pruning first when it nears the cap, and holds what was written; an error, holding the file as it was,
    /// when it cannot be saved.
    pub(crate) fn commit(&self, state: &mut State, next: DelegationFile, context: &DelegationPruneContext) -> Result<(), SaveError> {
        let written = self.inner.store.save_pruned(&next, context)?;
        self.set_file(state, written);
        Ok(())
    }

    /// `SaveOrKeep`: saves `next`; should the disk refuse it, the app goes on with it and the next save writes it.
    pub(crate) fn save_or_keep(&self, state: &mut State, next: DelegationFile, context: &DelegationPruneContext) {
        match self.inner.store.save_pruned(&next, context) {
            Ok(written) => self.set_file(state, written),
            Err(_) => self.set_file(state, next),
        }
    }

    /// `Save`: saves the file as the coordinator holds it now — what a save the disk refused earlier left unwritten is written.
    pub async fn save(&self) -> Result<(), SaveError> {
        let context = self.prune_context().await;
        let mut state = self.lock();
        let file = state.file.clone();
        self.commit(&mut state, file, &context)
    }

    /// `PruneContext`: what pruning may assume now — which of the file's parents still have an open pane, and that any merge of a child
    /// not discarded may still be undone, so no save here ever drops one.
    pub(crate) async fn prune_context(&self) -> DelegationPruneContext {
        let parents: Vec<String> = {
            let state = self.lock();
            let mut parents: Vec<String> = vec![];
            for child in &state.file.children {
                if !parents.contains(&child.parent_session_id) {
                    parents.push(child.parent_session_id.clone());
                }
            }
            parents
        };
        let mut open = HashSet::new();
        for parent in parents {
            if self.host_pane_state(&parent).await.is_some() {
                open.insert(parent);
            }
        }
        let discarded: HashSet<String> =
            self.lock().file.children.iter().filter(|c| c.state == ChildState::Discarded).map(|c| c.id.clone()).collect();
        DelegationPruneContext::new(open, move |merge| !discarded.contains(&merge.child_id))
    }

    // ── the host, each call on a task of its own ───────────────────────────────────────────────────────────────────────

    pub(crate) async fn host_pane_state(&self, session_id: &str) -> Option<DelegationPaneState> {
        let (host, id) = (self.inner.host.clone(), session_id.to_string());
        host_task(async move { host.pane_state(id).await }).await.flatten()
    }

    pub(crate) async fn host_deliver(&self, input: &str, session_id: &str, route: DeliveryRoute) -> Option<String> {
        let (host, input, id) = (self.inner.host.clone(), input.to_string(), session_id.to_string());
        host_task(async move { host.deliver(input, id, route).await }).await.flatten()
    }

    pub(crate) async fn host_start_run(&self, session_id: &str, input: &str) -> Option<String> {
        let (host, id, input) = (self.inner.host.clone(), session_id.to_string(), input.to_string());
        host_task(async move { host.start_run(id, input).await }).await.flatten()
    }

    pub(crate) async fn host_create_pane(&self, pane: crate::host::DelegationChildPane) -> bool {
        let host = self.inner.host.clone();
        host_task(async move { host.create_pane(pane).await }).await.unwrap_or(false)
    }

    pub(crate) async fn host_stop_run(&self, session_id: &str) {
        let (host, id) = (self.inner.host.clone(), session_id.to_string());
        host_task(async move { host.stop_run(id).await }).await;
    }

    pub(crate) async fn host_note(&self, text: String, session_id: &str) {
        let (host, id) = (self.inner.host.clone(), session_id.to_string());
        host_task(async move { host.note(text, id).await }).await;
    }

    pub(crate) async fn host_agent_availability(&self, provider: &str) -> crate::host::DelegationAgentAvailability {
        let (host, provider) = (self.inner.host.clone(), provider.to_string());
        host_task(async move { host.agent_availability(provider).await }).await.unwrap_or_default()
    }

    // ── the calls ──────────────────────────────────────────────────────────────────────────────────────────────────────

    /// `Handle`: one delegation call from `caller`, answered by the first rule that applies (see the type).
    pub async fn handle_call(&self, request: DelegationRequest, caller: DelegationCaller) -> DelegationResponse {
        if let Some(gated) = crate::providers::gate(&caller, (self.inner.switch_on)()) {
            return gated;
        }
        let Some(tool) = tools::named(&request.tool) else { return DelegationResponse::failure(unknown_tool_message(&request.tool)) };
        if tool.name == tools::DISCARD.name {
            return DelegationResponse::refusal(DelegationReasonCode::DiscardHumanOnly);
        }
        if let Some(problem) = argument_problem(request.arguments.keys(), tool) {
            return DelegationResponse::failure(problem);
        }
        let seconds = self.inner.answer_seconds;
        let late = DelegationResponse::failure(late_message(tool.name, seconds));
        let coordinator = self.clone();
        answer(seconds, late, async move { coordinator.perform(tool, request, caller).await }).await
    }

    /// `Perform`: runs one checked call from the pane `caller`.
    async fn perform(&self, tool: &'static Tool, request: DelegationRequest, caller: DelegationCaller) -> DelegationResponse {
        let value = |name: &str| request.arguments.get(name).cloned().unwrap_or_default();
        // Each tool's future on the heap: nested whole they make one future too large for a debug build's stack.
        match tool.name {
            "delegate" => {
                Box::pin(self.delegate_now(&value("task"), &value("mode"), request.arguments.get("agent").map(String::as_str), &caller))
                    .await
            }
            "list_children" => self.list_children(&caller),
            "child_status" => self.child_status(&value("child"), &caller),
            "merge" => Box::pin(self.merge_now(&value("child"), &value("expected_head"), &caller)).await,
            "follow_up" => Box::pin(self.follow_up_now(&value("child"), &value("text"), &caller)).await,
            // discard, which the call already refused: only a human discards.
            _ => DelegationResponse::refusal(DelegationReasonCode::DiscardHumanOnly),
        }
    }
}

impl DelegationHandler for Coordinator {
    fn handle(&self, request: DelegationRequest, caller: DelegationCaller) -> BoxFuture<'static, DelegationResponse> {
        let coordinator = self.clone();
        Box::pin(async move { coordinator.handle_call(request, caller).await })
    }
}
