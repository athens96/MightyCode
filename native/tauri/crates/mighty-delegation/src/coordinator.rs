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

/// What the coordinator holds that changes, read and changed only under its lock (held across a save's disk writes, as C#'s turn is).
pub(crate) struct State {
    pub(crate) file: Arc<DelegationFile>,
    /// Whether a delivery pass is offering pending items, and whether another pass was asked for meanwhile.
    pub(crate) is_delivering: bool,
    pub(crate) wants_delivery: bool,
    /// The file changed under this hold: the rows are published once the lock is let go.
    changed: bool,
}

/// What the coordinator keeps beside its file that readers ask for without waiting on a save (C#'s `PendingWork` reads them outside the
/// turn); its lock is only ever taken alone or inside the state's, and held for no IO.
#[derive(Default)]
pub(crate) struct Flags {
    /// The report revision at which each child was last asked to resolve its conflict with the parent branch.
    pub(crate) conflict_asked: HashMap<String, i32>,
    /// Children whose start is still going: delegate saved their record and their start has not finished. A discard waits for it.
    pub(crate) starting: HashSet<String>,
    /// The panes whose checkout a card's merge or undo is changing: delivery starts no run in them until it is over.
    pub(crate) card_panes: HashSet<String>,
}

/// The state's lock: what the hold changed in the file is published to the rows' watchers after the lock is let go, so a watcher that
/// holds a row's borrow never holds the coordinator up.
pub(crate) struct StateGuard<'a> {
    guard: Option<MutexGuard<'a, State>>,
    coordinator: &'a Coordinator,
}

impl std::ops::Deref for StateGuard<'_> {
    type Target = State;
    fn deref(&self) -> &State {
        self.guard.as_ref().expect("the state is held")
    }
}

impl std::ops::DerefMut for StateGuard<'_> {
    fn deref_mut(&mut self) -> &mut State {
        self.guard.as_mut().expect("the state is held")
    }
}

impl Drop for StateGuard<'_> {
    fn drop(&mut self) {
        let Some(mut guard) = self.guard.take() else { return };
        let changed = std::mem::take(&mut guard.changed);
        drop(guard);
        if changed {
            self.coordinator.publish();
        }
    }
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
    pub(crate) flags: Mutex<Flags>,
    /// The file as last set, for the readers that must not wait on a save.
    snapshot: Mutex<Arc<DelegationFile>>,
    /// One publication of the rows at a time, each from the newest file.
    publishing: Mutex<()>,
    pub(crate) ending_runs: ChildRunsEnding,
    /// A merge (the tool's or a card's) or a card's undo or discard: they take turns, so two never run git in a parent's checkout at
    /// once.
    pub(crate) merge_turns: tokio::sync::Mutex<()>,
    /// A run event of a child's pane (its start, its end, the parent awaiting it, or the launch's sweep): each is recorded whole before
    /// the next.
    pub(crate) run_turns: tokio::sync::Mutex<()>,
    /// MC-38: the children whose run an earlier app kept running and this launch took back: the launch's sweep of runs quitting
    /// killed leaves them running.
    pub(crate) adopted_runs: Mutex<std::collections::HashSet<String>>,
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
        let file = Arc::new(file);
        Ok(Coordinator {
            inner: Arc::new(Inner {
                store,
                host,
                worktrees: options.worktrees,
                paths: options.paths,
                switch_on: options.switch_on,
                answer_seconds: options.answer_seconds.min(ANSWER_SECONDS),
                locale: options.locale,
                state: Mutex::new(State { file: file.clone(), is_delivering: false, wants_delivery: false, changed: false }),
                flags: Mutex::new(Flags::default()),
                snapshot: Mutex::new(file),
                publishing: Mutex::new(()),
                ending_runs: ChildRunsEnding::default(),
                merge_turns: tokio::sync::Mutex::new(()),
                run_turns: tokio::sync::Mutex::new(()),
                adopted_runs: Mutex::new(std::collections::HashSet::new()),
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
    /// Read without waiting on a save that is going (the snapshot is swapped whole), and shared, not copied.
    pub fn file(&self) -> Arc<DelegationFile> {
        self.inner.snapshot.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).clone()
    }

    /// The children as the sidebar lists them now ([`sidebar::rows`]).
    pub fn rows(&self) -> Vec<DelegationChildRow> {
        sidebar::rows(&self.file())
    }

    /// `ChildRows`: the rows at load, then after each change to them; only the newest waits. A new receiver has the current rows waiting
    /// (C#'s channel holds the list at load for its reader).
    pub fn child_rows(&self) -> watch::Receiver<Vec<DelegationChildRow>> {
        let mut receiver = self.inner.rows.subscribe();
        receiver.mark_changed();
        receiver
    }

    /// `HeldRows`: the items held for each pane, oldest first, at load and after each change; only the newest waits. A new receiver has
    /// the current map waiting.
    pub fn held_rows(&self) -> watch::Receiver<IndexMap<String, Vec<DelegationDeliveryItem>>> {
        let mut receiver = self.inner.held_rows.subscribe();
        receiver.mark_changed();
        receiver
    }

    /// `PendingWork`: delegation work a restart would cut short — notices and follow-ups waiting to be offered, children still starting,
    /// the panes a card's merge or undo is changing, and a merge, undo or discard going. The automatic app update waits while it is
    /// above zero.
    /// Read without waiting on a save: the counts may be one change behind, which the next idle tick reads again (as in C#).
    pub fn pending_work(&self) -> usize {
        let file = self.file();
        let pending = file.notices.iter().filter(|n| n.lane == DeliveryLane::Pending).count()
            + file.follow_ups.iter().filter(|f| f.lane == DeliveryLane::Pending).count();
        let flags = self.flags();
        pending + flags.starting.len() + flags.card_panes.len() + usize::from(self.inner.merge_turns.try_lock().is_err())
    }

    /// `IsStarting`: the child's start is still going (delegate answered; its worktree, pane and first run are not all settled yet).
    pub fn is_starting(&self, id: &str) -> bool {
        self.flags().starting.contains(id)
    }

    pub(crate) fn lock(&self) -> StateGuard<'_> {
        StateGuard { guard: Some(self.inner.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner())), coordinator: self }
    }

    pub(crate) fn flags(&self) -> MutexGuard<'_, Flags> {
        self.inner.flags.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Publishes the rows and held rows of the newest file, each only when it differs from what the watchers have. Called with the state
    /// let go; publications take turns, each reading the file anew, so an older one never lands after a newer one.
    fn publish(&self) {
        let differ = |file: &DelegationFile| {
            *self.inner.rows.borrow() != sidebar::rows(file) || *self.inner.held_rows.borrow() != file.held_items_by_pane()
        };
        if !differ(&self.file()) {
            return;
        }
        let _turn = self.inner.publishing.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let file = self.file();
        let rows = sidebar::rows(&file);
        if *self.inner.rows.borrow() != rows {
            self.inner.rows.send_replace(rows);
        }
        let held = file.held_items_by_pane();
        if *self.inner.held_rows.borrow() != held {
            self.inner.held_rows.send_replace(held);
        }
    }

    pub(crate) fn locale(&self) -> Locale {
        (self.inner.locale)()
    }

    /// The file changed: readers see it at once, and the rows follow once the state is let go.
    fn set_file(&self, state: &mut State, file: DelegationFile) {
        let file = Arc::new(file);
        state.file = file.clone();
        state.changed = true;
        *self.inner.snapshot.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = file;
    }

    /// The store's save, off tokio's async workers when it may wait on the disk (40 tries 50 ms apart): on a multi-threaded runtime the
    /// worker hands its other tasks on while this one blocks (a current-thread runtime, as the checks run, blocks as C#'s turn does).
    fn save_blocking(&self, next: &DelegationFile, context: &DelegationPruneContext) -> Result<DelegationFile, SaveError> {
        let save = || self.inner.store.save_pruned(next, context);
        match tokio::runtime::Handle::try_current() {
            Ok(handle) if handle.runtime_flavor() == tokio::runtime::RuntimeFlavor::MultiThread => tokio::task::block_in_place(save),
            _ => save(),
        }
    }

    /// `File = next` without a save (a change the disk refused, held until the next save).
    pub(crate) fn keep(&self, state: &mut State, next: DelegationFile) {
        self.set_file(state, next);
    }

    /// `Commit`: saves `next`, pruning first when it nears the cap, and holds what was written; an error, holding the file as it was,
    /// when it cannot be saved.
    pub(crate) fn commit(&self, state: &mut State, next: DelegationFile, context: &DelegationPruneContext) -> Result<(), SaveError> {
        let written = self.save_blocking(&next, context)?;
        self.set_file(state, written);
        Ok(())
    }

    /// `SaveOrKeep`: saves `next`; should the disk refuse it, the app goes on with it and the next save writes it.
    pub(crate) fn save_or_keep(&self, state: &mut State, next: DelegationFile, context: &DelegationPruneContext) {
        match self.save_blocking(&next, context) {
            Ok(written) => self.set_file(state, written),
            Err(_) => self.set_file(state, next),
        }
    }

    /// `Save`: saves the file as the coordinator holds it now — what a save the disk refused earlier left unwritten is written.
    pub async fn save(&self) -> Result<(), SaveError> {
        self.isolated(
            |reason| Err(SaveError::Io(std::io::Error::new(std::io::ErrorKind::Interrupted, reason))),
            |coordinator| async move {
                let context = coordinator.prune_context().await;
                let mut state = coordinator.lock();
                let file = DelegationFile::clone(&state.file);
                coordinator.commit(&mut state, file, &context)
            },
        )
        .await
    }

    /// C#'s `Isolated`: a mutating call runs to its end on a task of its own, whatever happens to the caller's future (a dropped call
    /// never leaves a merge made in git but not on record). A panic in it is the caller's; a task cancelled when its runtime shuts down
    /// returns `cancelled` instead of panicking.
    pub(crate) async fn isolated<T, F>(&self, cancelled: impl FnOnce(String) -> T, work: impl FnOnce(Coordinator) -> F) -> T
    where
        T: Send + 'static,
        F: Future<Output = T> + Send + 'static,
    {
        await_isolated(tokio::spawn(work(self.clone())), cancelled).await
    }

    /// `PruneContext`: what pruning may assume now — which of the file's parents still have an open pane, and that any merge of a child
    /// not discarded may still be undone, so no save here ever drops one.
    pub(crate) async fn prune_context(&self) -> DelegationPruneContext {
        let parents: Vec<String> = {
            let file = self.file();
            let mut parents: Vec<String> = vec![];
            for child in &file.children {
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
            self.file().children.iter().filter(|c| c.state == ChildState::Discarded).map(|c| c.id.clone()).collect();
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

/// The public mutating seams: each runs to its end on a task of its own ([`Coordinator::isolated`], C#'s `Isolated`), so a caller that
/// drops its future never cuts a merge, a save or a run event short. Run events (`child_run_started`, `child_run_ended`, `await_run`)
/// take their FIFO turn when their task first runs, not when they are called: a caller that needs them recorded in order awaits each
/// before sending the next, as the app's event pump does.
impl Coordinator {
    /// `MergeFromCard` ([`crate::cards`]).
    pub async fn merge_from_card(&self, id: &str) -> crate::git::merge::ChildMergeOutcome {
        let id = id.to_string();
        self.isolated(
            crate::git::merge::ChildMergeOutcome::Failed,
            move |coordinator| async move { coordinator.merge_from_card_now(&id).await },
        )
        .await
    }

    /// `UndoMergeFromCard` ([`crate::cards`]).
    pub async fn undo_merge_from_card(&self, id: &str) -> crate::git::merge::ChildUndoOutcome {
        let id = id.to_string();
        self.isolated(crate::git::merge::ChildUndoOutcome::Failed, move |coordinator| async move {
            coordinator.undo_merge_from_card_now(&id).await
        })
        .await
    }

    /// `DiscardFromCard` ([`crate::cards`]).
    pub async fn discard_from_card(&self, id: &str, confirmed_nested: Option<&[String]>) -> crate::cards::ChildCardDiscardOutcome {
        let (id, confirmed) = (id.to_string(), confirmed_nested.map(<[String]>::to_vec));
        self.isolated(crate::cards::ChildCardDiscardOutcome::Failed, move |coordinator| async move {
            coordinator.discard_from_card_now(&id, confirmed).await
        })
        .await
    }

    /// `CloseChild` ([`crate::cards`]).
    pub async fn close_child(&self, id: &str, still_running: bool) -> crate::git::cleanup::ChildCleanupOutcome {
        let id = id.to_string();
        self.isolated(crate::git::cleanup::ChildCleanupOutcome::Failed, move |coordinator| async move {
            coordinator.close_child_now(&id, still_running).await
        })
        .await
    }

    /// `ChildRunStarted` ([`crate::reports`]).
    pub async fn child_run_started(&self, id: &str, run_id: &str) {
        let (id, run_id) = (id.to_string(), run_id.to_string());
        self.isolated(|_| (), move |coordinator| async move { coordinator.child_run_started_now(&id, &run_id).await }).await
    }

    /// MC-38: a run an earlier app kept running goes on in the child `id`'s pane as `run_id` ([`crate::reports`]).
    pub async fn child_run_adopted(&self, id: &str, run_id: &str) {
        let (id, run_id) = (id.to_string(), run_id.to_string());
        self.isolated(|_| (), move |coordinator| async move { coordinator.child_run_adopted_now(&id, &run_id).await }).await
    }

    /// `ChildRunEnded` ([`crate::reports`]).
    pub async fn child_run_ended(&self, id: &str, run_id: &str, end: crate::reports::ChildRunEnd) -> Option<crate::model::Notice> {
        let (id, run_id) = (id.to_string(), run_id.to_string());
        self.isolated(|_| None, move |coordinator| async move { coordinator.child_run_ended_now(&id, &run_id, end).await }).await
    }

    /// `AwaitRun` ([`crate::reports`]).
    pub async fn await_run(&self, run_id: &str, id: &str) -> Option<crate::model::Notice> {
        let (run_id, id) = (run_id.to_string(), id.to_string());
        self.isolated(|_| None, move |coordinator| async move { coordinator.await_run_now(&run_id, &id).await }).await
    }

    /// `InterruptRunsKilledByQuit` ([`crate::reports`]).
    pub async fn interrupt_runs_killed_by_quit(&self) -> Vec<String> {
        self.isolated(|_| Vec::new(), |coordinator| async move { coordinator.interrupt_runs_killed_by_quit_now().await }).await
    }

    /// `Send` ([`crate::delivery`]).
    pub async fn send(&self, text: &str, session_id: &str, start: Option<RunStarter>) -> crate::delivery::DelegationRelease {
        let (text, session_id) = (text.to_string(), session_id.to_string());
        self.isolated(
            |_| crate::delivery::DelegationRelease::NotStarted,
            move |coordinator| async move { coordinator.send_now(&text, &session_id, start).await },
        )
        .await
    }

    /// `RunNext` ([`crate::delivery`]).
    pub async fn run_next(&self, session_id: &str) -> crate::delivery::DelegationRelease {
        let session_id = session_id.to_string();
        self.isolated(
            |_| crate::delivery::DelegationRelease::NotStarted,
            move |coordinator| async move { coordinator.run_next_now(&session_id).await },
        )
        .await
    }

    /// `StartHumanChild` ([`crate::tools::delegate`]).
    pub async fn start_human_child(
        &self,
        text: &str,
        parent_id: &str,
        starter: crate::tools::delegate::HumanChildStarter,
        plan_first: bool,
        agent: Option<&str>,
    ) -> crate::tools::delegate::HumanChildStart {
        self.start_human_child_at(text, parent_id, starter, plan_first, agent, None).await
    }

    /// [`Coordinator::start_human_child`] from `base_commit` (MC-29's compare run): the head or an earlier commit of the parent's
    /// branch; None is the head, as the second send has it.
    pub async fn start_human_child_at(
        &self,
        text: &str,
        parent_id: &str,
        starter: crate::tools::delegate::HumanChildStarter,
        plan_first: bool,
        agent: Option<&str>,
        base_commit: Option<&str>,
    ) -> crate::tools::delegate::HumanChildStart {
        let (text, parent_id, agent, base) =
            (text.to_string(), parent_id.to_string(), agent.map(str::to_string), base_commit.map(str::to_string));
        self.isolated(crate::tools::delegate::HumanChildStart::Failed, move |coordinator| async move {
            coordinator.start_human_child_now(&text, &parent_id, starter, plan_first, agent.as_deref(), base.as_deref()).await
        })
        .await
    }
}

async fn await_isolated<T>(task: tokio::task::JoinHandle<T>, cancelled: impl FnOnce(String) -> T) -> T {
    match task.await {
        Ok(value) => value,
        Err(error) => match error.try_into_panic() {
            Ok(panic) => std::panic::resume_unwind(panic),
            Err(error) => cancelled(format!("the coordinator's task did not finish: {error}")),
        },
    }
}

impl DelegationHandler for Coordinator {
    fn handle(&self, request: DelegationRequest, caller: DelegationCaller) -> BoxFuture<'static, DelegationResponse> {
        let coordinator = self.clone();
        Box::pin(async move { coordinator.handle_call(request, caller).await })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn cancelled_isolated_task_returns_fallback_without_panicking() {
        let handle = tokio::spawn(async { std::future::pending::<Result<(), String>>().await });
        handle.abort();
        let outcome = await_isolated(handle, Err).await;
        assert!(outcome.is_err_and(|msg| msg.contains("the coordinator's task did not finish")));
    }
}
