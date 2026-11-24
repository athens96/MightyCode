//! The remote mirror's client side in the app (C# `DesktopService.Mirror`, M/MightyCode/AppStore+RemoteMirror.swift): each connected
//! computer's shared workspaces and agent panes, held in the state while connected and never saved (the store leaves them out), kept current
//! by one poll per connection, and driven through the host. A connection that comes up starts its mirror, one that goes down stops it (the
//! panes stay as last shown, sends wait for a new connect), one removed takes its mirror away.
//!
//! The app's state is reached through [`MirrorStage`] (src-tauri mirror.rs over the run service; the checks over an in-memory state).
//! Locks: the desk's own book is never held while the stage is called. The stage's change closure may take the book briefly (the state's
//! lock, then the book: [`MirrorDesk::apply_state`]), never the other way round.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak};
use std::time::{Duration, Instant};

use indexmap::IndexMap;
use mighty_core::attachment::RunAttachment;
use mighty_core::model::{AppSnapshot, MirrorReference, RemoteWorkspaceReference, RunSession, Workspace};
use mighty_core::permission::ToolPermissionRequest;
use mighty_core::time::DateTimeOffset;
use mighty_platform::BoxFuture;
use mighty_run::plan_mode::PlanDecision;
use mighty_run::questionnaire::UserQuestionAnswer;
use mighty_run::service::MirrorRunner;
use mighty_run::strings::{get, get_with};
use mighty_run::{Result as RunResult, RunError};
use tokio::sync::Notify;

use super::client::{bad_data, refusal_status, transient};
use super::{MirrorPane, MirrorQueued, MirrorState, MirrorSummary, protocol, support};
use crate::remote::client::{ClientError, RemoteClient};
use crate::remote::wire::RemoteConnectionStatus;

/// The app's state as the mirror reads and changes it.
///
/// Contract: every method may be called from any task; none is called while the desk holds its own lock. [`MirrorStage::change`] applies a
/// change under the state's lock, normalized as every change is and never saved (C#'s `snapshot = StateStore.Normalize(next, false)`);
/// `change` gives None to change nothing. The other three are C#'s events, raised outside every lock.
pub trait MirrorStage: Send + Sync {
    /// `Snapshot`: the state as it is now.
    fn snapshot(&self) -> AppSnapshot;
    /// The mirror's own change to the state; false when nothing changed (or the app is closing).
    fn change(&self, change: &mut dyn FnMut(&AppSnapshot) -> Option<AppSnapshot>) -> bool;
    /// `ToolPermissionChanged`: a mirrored pane's waiting request shown as a card (its run id the pane's), or withdrawn.
    fn permission(&self, request: ToolPermissionRequest);
    /// `MirrorChanged`: a mirror's state, a mirrored pane's counts or its workspaces changed (the sidebar and the strips are drawn again).
    fn changed(&self);
    /// `MirrorFailed(pane, words)`: an action on a mirrored pane failed on the host; the words are the host's (or the connection's).
    fn failed(&self, pane_id: &str, words: &str);
}

/// Where the desk runs its polls and answers (the app's runtime; the checks' tokio).
pub type Spawn = Arc<dyn Fn(BoxFuture<'static, ()>) + Send + Sync>;

/// C# `RemoteMirrorLink(Phase, Error?, Unsupported)`: how one connection's mirror goes: `live` (reading the host's list), `reconnecting`
/// (asking again after a failed read) or `off`. `unsupported`: the host answered the mirror's list 404 (an older Mighty Code): its shared
/// workspaces still open as this PC's own remote panes (Settings' 열기).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MirrorLink {
    pub phase: &'static str,
    pub error: Option<String>,
    pub unsupported: bool,
}

impl MirrorLink {
    fn of(phase: &'static str) -> MirrorLink {
        MirrorLink { phase, error: None, unsupported: false }
    }
}

/// C# `RemoteMirrorCounts`: what a mirrored pane shows beyond its record: the host's waiting work and branch from its list, the queued
/// requests (id, text and what 바로 실행 does there) and base from its detail. `queued_items` is None until a detail was read, and empty
/// as soon as the list says the queue is; `queue_actions`: the host takes the queue's actions (MC-49; an older host does not).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MirrorCounts {
    pub pending: i64,
    pub queued: i64,
    pub queued_items: Option<Vec<MirrorQueued>>,
    pub queue_actions: bool,
    pub branch: Option<String>,
    pub base: Option<String>,
}

impl MirrorCounts {
    /// The queued requests' texts, in the host's order (the strip's tooltip).
    pub fn queued_texts(&self) -> Option<Vec<String>> {
        self.queued_items.as_ref().map(|items| items.iter().map(|q| q.text.clone()).collect())
    }
}

/// MC-49: an action on a queued request of a mirrored pane, taken on its host.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueueAction {
    /// × on the row: the request leaves the host pane's queue.
    Remove,
    /// 바로 실행 on the row: the host runs it now, as its own 바로 실행 would.
    RunNow,
}

/// Set once to end a poll; every wait of the poll ends with it.
#[derive(Default)]
struct Stop {
    set: AtomicBool,
    notify: Notify,
}

impl Stop {
    fn stop(&self) {
        self.set.store(true, Ordering::SeqCst);
        self.notify.notify_waiters();
    }
    fn is_set(&self) -> bool {
        self.set.load(Ordering::SeqCst)
    }
    async fn wait(&self) {
        loop {
            let notified = self.notify.notified();
            if self.is_set() {
                return;
            }
            notified.await;
        }
    }
}

/// C#'s `MirrorPoll`: one connection's poll. `serial` tells this poll from a later one under the same connection.
struct Poll {
    serial: u64,
    link: MirrorLink,
    revision: Option<String>,
    stop: Arc<Stop>,
    wake: Arc<AtomicBool>,
    done: Arc<AtomicBool>,
}

#[derive(Default)]
struct Book {
    polls: HashMap<String, Poll>,
    summary_revisions: HashMap<String, String>,
    detail_revisions: HashMap<String, String>,
    counts: HashMap<String, MirrorCounts>,
    /// A mirrored pane's waiting requests as its cards show them (run id the pane's), each with the host's run id to answer with.
    requests: HashMap<String, Vec<(ToolPermissionRequest, String)>>,
    /// The status each connection had when last seen.
    seen: HashMap<String, RemoteConnectionStatus>,
    closing: bool,
}

struct Inner {
    client: RemoteClient,
    stage: Arc<dyn MirrorStage>,
    spawn: Spawn,
    book: Mutex<Book>,
    serials: AtomicU64,
    /// How a poll waits between reads: the Mac's 1 s and 5 s (the checks shorten them).
    intervals: Mutex<(Duration, Duration)>,
}

/// The remote mirror's client side.
#[derive(Clone)]
pub struct MirrorDesk(Arc<Inner>);

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// What an answer to a mirrored pane's request carries.
enum Answer {
    Permission(bool),
    Questions(IndexMap<String, UserQuestionAnswer>),
    Plan(PlanDecision),
}

fn mirror_of(workspace: &Workspace) -> Option<&RemoteWorkspaceReference> {
    workspace.remote.as_ref().filter(|r| r.mirror)
}

fn connection_of(workspace: &Workspace) -> Option<&str> {
    mirror_of(workspace).and_then(|r| r.connection_id.as_deref())
}

/// `ApplyMirrorState`'s change: the host's list laid over this PC's state: a workspace per shared one, a pane per host pane, the ones
/// gone taken away. The next state (not yet normalized), the panes gone, and each listed pane's client id with its summary.
pub fn state_applied(
    snapshot: &AppSnapshot,
    connection_id: &str,
    state: &MirrorState,
) -> (AppSnapshot, Vec<String>, Vec<(String, MirrorSummary)>) {
    let workspace_ids: HashMap<&str, String> =
        state.workspaces.iter().map(|w| (w.id.as_str(), support::client_id(connection_id, &w.id))).collect();
    let kept: HashSet<&str> = workspace_ids.values().map(String::as_str).collect();
    let dropped: HashSet<String> = snapshot
        .workspaces()
        .iter()
        .filter(|w| connection_of(w) == Some(connection_id) && !kept.contains(w.id()))
        .map(|w| w.id().to_string())
        .collect();
    let mut workspaces: Vec<Workspace> = snapshot.workspaces().iter().filter(|w| !dropped.contains(w.id())).cloned().collect();
    for shared in &state.workspaces {
        let client_id = &workspace_ids[shared.id.as_str()];
        let reference = RemoteWorkspaceReference {
            connection_id: Some(connection_id.into()),
            workspace_id: Some(shared.id.clone()),
            host_name: Some(state.host_name.clone()),
            version: 2,
            mirror: true,
        };
        match workspaces.iter_mut().find(|w| w.id() == client_id) {
            Some(listed) => {
                listed.name = Some(shared.name.clone());
                listed.path = Some(shared.path.clone());
                listed.remote = Some(reference);
            }
            None => workspaces.push(Workspace {
                id: Some(client_id.clone()),
                name: Some(shared.name.clone()),
                path: Some(shared.path.clone()),
                remote: Some(reference),
                ..Workspace::default()
            }),
        }
    }
    let mut sessions: Vec<Arc<RunSession>> = snapshot.sessions().to_vec();
    let mut kept_sessions: HashSet<String> = HashSet::new();
    let mut listed = vec![];
    for summary in &state.sessions {
        let Some(workspace_id) = workspace_ids.get(summary.workspace_id.as_str()) else { continue };
        let client_id = support::client_id(connection_id, &summary.id);
        kept_sessions.insert(client_id.clone());
        let reference = MirrorReference {
            connection_id: connection_id.into(),
            host_session_id: summary.id.clone(),
            host_workspace_id: summary.workspace_id.clone(),
            host_name: state.host_name.clone(),
        };
        let at = sessions.iter().position(|s| s.id() == client_id);
        let pane = support::pane(summary, at.map(|i| &*sessions[i]), &client_id, workspace_id, reference);
        match at {
            Some(i) => {
                if *sessions[i] != pane {
                    sessions[i] = Arc::new(pane);
                }
            }
            None => sessions.push(Arc::new(pane)),
        }
        listed.push((client_id, summary.clone()));
    }
    let gone: Vec<String> = sessions
        .iter()
        .filter(|s| {
            s.mirror.as_ref().is_some_and(|m| m.connection_id == connection_id) && !kept_sessions.contains(s.id())
                || dropped.contains(s.workspace_id())
        })
        .map(|s| s.id().to_string())
        .collect();
    sessions.retain(|s| !gone.iter().any(|g| g == s.id()));
    let mut next = AppSnapshot { workspaces: Some(workspaces), sessions: Some(sessions), ..snapshot.clone() };
    // A workspace the host stopped sharing, or a pane closed there, stops being the one shown.
    if next.active_workspace_id.as_ref().is_some_and(|active| dropped.contains(active)) {
        let shown = next.workspaces().iter().find(|w| w.remote.is_none()).or(next.workspaces().first()).map(|w| w.id().to_string());
        next.active_session_id = next.sessions().iter().find(|s| Some(s.workspace_id()) == shown.as_deref()).map(|s| s.id().to_string());
        next.active_workspace_id = shown;
    } else if next.active_session_id.as_ref().is_some_and(|pane| gone.contains(pane)) {
        let active = next.active_workspace_id.clone();
        next.active_session_id = next.sessions().iter().find(|s| Some(s.workspace_id()) == active.as_deref()).map(|s| s.id().to_string());
    }
    (next, gone, listed)
}

impl MirrorDesk {
    /// A desk over `client` and `stage`; its polls and answers run on `spawn`.
    pub fn new(client: RemoteClient, stage: Arc<dyn MirrorStage>, spawn: Spawn) -> MirrorDesk {
        MirrorDesk(Arc::new(Inner {
            client,
            stage,
            spawn,
            book: Mutex::new(Book::default()),
            serials: AtomicU64::new(1),
            intervals: Mutex::new((protocol::LIVE_POLL_INTERVAL, protocol::IDLE_POLL_INTERVAL)),
        }))
    }

    /// `MirrorLiveInterval`, `MirrorIdleInterval` (the checks shorten them).
    pub fn set_intervals(&self, live: Duration, idle: Duration) {
        *lock(&self.0.intervals) = (live, idle);
    }

    /// C#'s `Remote.Changed += RemoteConnectionsChanged`: from now on a connection that comes up starts its mirror, one that goes down
    /// stops it, one removed takes its mirror away (worked out on a task of its own, off the client's).
    pub fn attach(&self) {
        let weak: Weak<Inner> = Arc::downgrade(&self.0);
        self.0.client.on_changed(Arc::new(move || {
            let Some(inner) = weak.upgrade() else { return };
            let desk = MirrorDesk(inner.clone());
            (inner.spawn)(Box::pin(async move { desk.connections_changed() }));
        }));
    }

    pub fn client(&self) -> &RemoteClient {
        &self.0.client
    }

    fn book(&self) -> MutexGuard<'_, Book> {
        lock(&self.0.book)
    }

    /// `MirrorLink(connectionId)`.
    pub fn link(&self, connection_id: &str) -> Option<MirrorLink> {
        self.book().polls.get(connection_id).map(|p| p.link.clone())
    }

    /// `MirrorCounts(paneId)`.
    pub fn counts(&self, pane_id: &str) -> Option<MirrorCounts> {
        self.book().counts.get(pane_id).cloned()
    }

    /// `MirrorReference(paneId)`.
    pub fn reference(&self, pane_id: &str) -> Option<MirrorReference> {
        self.0.stage.snapshot().session(pane_id).and_then(|s| s.mirror.clone())
    }

    /// `IsMirrorWorkspace(workspaceId)`.
    pub fn is_mirror_workspace(&self, workspace_id: &str) -> bool {
        self.0.stage.snapshot().workspace(workspace_id).is_some_and(|w| mirror_of(w).is_some())
    }

    /// `MirrorBlockedReason(mirror)`: why a mirrored pane cannot send now: its computer is not connected; None when it can.
    pub fn blocked_reason(&self, mirror: &MirrorReference) -> Option<String> {
        let connected = self.0.client.connection(&mirror.connection_id).is_some_and(|c| c.status == RemoteConnectionStatus::Connected);
        (!connected).then(|| get_with("remote.mirror.error.disconnected", &[("host", &mirror.host_name)]))
    }

    /// `RemoteConnectionsChanged`: the client's connections changed.
    pub fn connections_changed(&self) {
        let list = self.0.client.connections();
        let (mut start, mut stop, mut removed) = (vec![], vec![], vec![]);
        {
            let mut book = self.book();
            if book.closing {
                return;
            }
            for connection in &list {
                let was = book.seen.get(&connection.id).copied().unwrap_or(RemoteConnectionStatus::Saved);
                book.seen.insert(connection.id.clone(), connection.status);
                let connected = connection.status == RemoteConnectionStatus::Connected;
                // Connected now: a new connection, a refresh after the mirror ended (not an older host's 404), or a key replaced meanwhile.
                let ended = book.polls.get(&connection.id).is_none_or(|p| p.done.load(Ordering::SeqCst) && !p.link.unsupported);
                if connected && (was != RemoteConnectionStatus::Connected || ended) {
                    start.push(connection.id.clone());
                } else if !connected && was == RemoteConnectionStatus::Connected {
                    stop.push(connection.id.clone());
                }
            }
            let gone: Vec<String> = book.seen.keys().filter(|id| list.iter().all(|c| &c.id != *id)).cloned().collect();
            for id in gone {
                book.seen.remove(&id);
                removed.push(id);
            }
        }
        for id in start {
            self.start(&id);
        }
        for id in stop {
            self.stop_mirror(&id);
        }
        for id in removed {
            self.remove_mirrors(&id);
        }
    }

    /// `StartMirror(connectionId)`: starts (or wakes) the connection's mirror: its workspaces and panes come into the sidebar and stay
    /// current.
    pub fn start(&self, connection_id: &str) {
        let serial = self.0.serials.fetch_add(1, Ordering::SeqCst);
        let (stop, wake, done) = (Arc::new(Stop::default()), Arc::new(AtomicBool::new(false)), Arc::new(AtomicBool::new(false)));
        {
            let mut book = self.book();
            if book.closing {
                return;
            }
            if let Some(running) = book.polls.get(connection_id).filter(|p| !p.done.load(Ordering::SeqCst)) {
                running.wake.store(true, Ordering::SeqCst);
                return;
            }
            book.polls.insert(
                connection_id.into(),
                Poll { serial, link: MirrorLink::of("live"), revision: None, stop: stop.clone(), wake: wake.clone(), done: done.clone() },
            );
        }
        let desk = self.clone();
        let id = connection_id.to_string();
        (self.0.spawn)(Box::pin(async move {
            desk.poll_loop(&id, serial, &stop, &wake).await;
            done.store(true, Ordering::SeqCst);
        }));
        self.0.stage.changed();
    }

    /// `StopMirror(connectionId)`: stops the connection's mirror (it was disconnected): its panes stay as last shown, and sends wait for a
    /// new connect.
    pub fn stop_mirror(&self, connection_id: &str) {
        {
            let mut book = self.book();
            let Some(poll) = book.polls.get_mut(connection_id) else { return };
            poll.stop.stop();
            if poll.link.phase != "off" {
                poll.link = MirrorLink::of("off");
            }
        }
        self.0.stage.changed();
    }

    /// `RemoveMirrors(connectionId)`: the connection's mirrored workspaces and panes go (the connection was removed).
    pub fn remove_mirrors(&self, connection_id: &str) {
        let removed = self.book().polls.remove(connection_id);
        if let Some(poll) = removed {
            poll.stop.stop();
        }
        let mut gone = vec![];
        self.0.stage.change(&mut |snapshot| {
            let workspaces: HashSet<String> =
                snapshot.workspaces().iter().filter(|w| connection_of(w) == Some(connection_id)).map(|w| w.id().to_string()).collect();
            gone = snapshot
                .sessions()
                .iter()
                .filter(|s| s.mirror.as_ref().is_some_and(|m| m.connection_id == connection_id) || workspaces.contains(s.workspace_id()))
                .map(|s| s.id().to_string())
                .collect();
            if workspaces.is_empty() && gone.is_empty() {
                return None;
            }
            let mut next = snapshot.clone();
            next.workspaces_mut().retain(|w| !workspaces.contains(w.id()));
            next.sessions_mut().retain(|s| !gone.iter().any(|g| g == s.id()));
            if next.active_workspace_id.as_ref().is_some_and(|active| workspaces.contains(active)) {
                next.active_workspace_id = next.workspaces().first().map(|w| w.id().to_string());
                next.active_session_id = None;
            }
            Some(next)
        });
        self.forget(&gone);
        self.0.stage.changed();
    }

    /// `WakeMirror(connectionId)`: the connection's poll reads at once.
    pub fn wake(&self, connection_id: &str) {
        let book = self.book();
        if let Some(poll) = book.polls.get(connection_id) {
            poll.wake.store(true, Ordering::SeqCst);
        }
    }

    /// `WakeMirrorFor(workspaceId)`: the user looks at another workspace or pane: a mirror workspace's poll reads at once.
    pub fn wake_for(&self, workspace_id: &str) {
        let snapshot = self.0.stage.snapshot();
        if let Some(connection) = snapshot.workspace(workspace_id).and_then(connection_of) {
            self.wake(connection);
        }
    }

    /// The poll's own entry, when it is still the connection's.
    fn with_poll<T>(&self, connection_id: &str, serial: u64, work: impl FnOnce(&mut Poll) -> T) -> Option<T> {
        let mut book = self.book();
        book.polls.get_mut(connection_id).filter(|p| p.serial == serial).map(work)
    }

    async fn poll_loop(&self, id: &str, serial: u64, stop: &Stop, wake: &AtomicBool) {
        let mut failing_since: Option<Instant> = None;
        while !stop.is_set() {
            let pass = tokio::select! {
                () = stop.wait() => break,
                pass = self.pass(id, serial, stop, &mut failing_since) => pass,
            };
            if let Err(error) = pass {
                // A host that answers but refuses the mirror (an older Mighty Code answers 404) stays connected: its workspaces still take
                // this PC's own remote panes, only the mirror stops.
                if let Some(status) = refusal_status(&error).filter(|s| *s != 401) {
                    let host = self.0.client.connection(id).map(|c| c.name).unwrap_or_default();
                    let words = if status == 404 { get_with("remote.mirror.error.olderHost", &[("host", &host)]) } else { error.message() };
                    self.with_poll(id, serial, |poll| {
                        poll.link = MirrorLink { phase: "off", error: Some(words), unsupported: status == 404 };
                        poll.revision = None;
                    });
                    self.0.stage.changed();
                    break;
                }
                // A busy host or a stumbling network is asked again for ten seconds (the client's retry window), as a remote run's poll is.
                let now = Instant::now();
                let window = self.0.client.0.options.poll_retry_window;
                if transient(&error) && now.duration_since(failing_since.unwrap_or(now)) < window {
                    failing_since.get_or_insert(now);
                    self.with_poll(id, serial, |poll| poll.link.phase = "reconnecting");
                    self.0.stage.changed();
                    tokio::select! {
                        () = stop.wait() => break,
                        () = tokio::time::sleep(Duration::from_secs(1)) => {}
                    }
                    continue;
                }
                // Past its retries, or an answer that is not the mirror's: the connection is lost, the mirror stops. A 401 the client already
                // took as disconnected, and a connection that went or changed meanwhile stands as it is.
                if transient(&error) || bad_data(&error) {
                    self.0.client.mirror_lost(id, &error.message()).await;
                }
                self.with_poll(id, serial, |poll| {
                    poll.link = MirrorLink { phase: "off", error: Some(error.message()), unsupported: false };
                    poll.revision = None;
                });
                self.0.stage.changed();
                break;
            }
            self.pause(id, stop, wake).await;
        }
    }

    /// One read of the host's list, laid over the state, and the details this PC needs now.
    async fn pass(&self, id: &str, serial: u64, stop: &Stop, failing_since: &mut Option<Instant>) -> Result<(), ClientError> {
        let since = self.with_poll(id, serial, |poll| poll.revision.clone()).flatten();
        let state = self.0.client.mirror_state(id, since.as_deref()).await?;
        if stop.is_set() {
            return Ok(());
        }
        *failing_since = None;
        if !state.unchanged {
            self.apply_state(id, serial, &state);
        }
        let moved = self
            .with_poll(id, serial, |poll| {
                let moved = poll.link.phase != "live" || poll.link.error.is_some();
                poll.link = MirrorLink::of("live");
                poll.revision = Some(state.revision.clone());
                moved
            })
            .unwrap_or(false);
        if moved || !state.unchanged {
            self.0.stage.changed();
        }
        self.fetch_details(id, serial, stop).await
    }

    /// `MirrorPauseAsync`: one second while a pane of this connection runs or waits, or the user looks at one of its workspaces; five
    /// otherwise. A selection, a send or an answer wakes it at once.
    async fn pause(&self, id: &str, stop: &Stop, wake: &AtomicBool) {
        let snapshot = self.0.stage.snapshot();
        let watching = snapshot.active_workspace_id.as_deref().and_then(|w| snapshot.workspace(w)).and_then(connection_of) == Some(id);
        let live = watching || {
            let book = self.book();
            snapshot.sessions().iter().any(|s| {
                s.mirror.as_ref().is_some_and(|m| m.connection_id == id)
                    && (s.status() == "running" || book.counts.get(s.id()).is_some_and(|c| c.pending > 0))
            })
        };
        let (live_interval, idle_interval) = *lock(&self.0.intervals);
        let until = Instant::now() + if live { live_interval } else { idle_interval };
        while Instant::now() < until && !stop.is_set() {
            if wake.swap(false, Ordering::SeqCst) {
                return;
            }
            tokio::select! {
                () = stop.wait() => return,
                () = tokio::time::sleep(Duration::from_millis(50)) => {}
            }
        }
    }

    /// `ApplyMirrorState(id, state)`: the host's list read by poll `serial` laid over this PC's. Never saved. Nothing is laid over once
    /// that poll is no longer the connection's or was stopped (the connection removed or disconnected while the list was read): checked
    /// under the book inside the state's change, so a removal's taking away is never undone by a read that was under way.
    pub fn apply_state(&self, id: &str, serial: u64, state: &MirrorState) {
        let (mut gone, mut listed) = (vec![], vec![]);
        let changed = self.0.stage.change(&mut |snapshot| {
            {
                let book = self.book();
                let current = book.polls.get(id).is_some_and(|p| p.serial == serial && !p.stop.is_set());
                if book.closing || !current {
                    return None;
                }
            }
            let (next, g, l) = state_applied(snapshot, id, state);
            gone = g;
            listed = l;
            Some(next)
        });
        if !changed {
            return;
        }
        {
            let mut book = self.book();
            for (client_id, summary) in &listed {
                book.summary_revisions.insert(client_id.clone(), summary.revision.clone());
                let counts = book.counts.get(client_id).cloned().unwrap_or_default();
                // The rows themselves come with the detail; an emptied queue is empty at once.
                let queued_items = if summary.queued == 0 { Some(vec![]) } else { counts.queued_items.clone() };
                book.counts.insert(
                    client_id.clone(),
                    MirrorCounts {
                        pending: summary.pending,
                        queued: summary.queued,
                        branch: summary.branch.clone(),
                        queued_items,
                        ..counts
                    },
                );
            }
        }
        self.forget(&gone);
    }

    /// `FetchMirrorDetailsAsync`: the details this PC needs now: the panes of the workspace the user looks at and any pane waiting for an
    /// answer, each only when its revision moved since it was read, at most 16 a pass.
    async fn fetch_details(&self, id: &str, serial: u64, stop: &Stop) -> Result<(), ClientError> {
        let snapshot = self.0.stage.snapshot();
        let wanted: Vec<(String, String)> = {
            let book = self.book();
            let watching = snapshot.active_workspace_id.clone();
            snapshot
                .sessions()
                .iter()
                .filter_map(|s| {
                    let mirror = s.mirror.as_ref().filter(|m| m.connection_id == id)?;
                    let looked_at = Some(s.workspace_id()) == watching.as_deref() || book.counts.get(s.id()).is_some_and(|c| c.pending > 0);
                    let moved = book.summary_revisions.get(s.id()) != book.detail_revisions.get(s.id());
                    (looked_at && moved).then(|| (s.id().to_string(), mirror.host_session_id.clone()))
                })
                .take(protocol::MAXIMUM_DETAILS_PER_PASS)
                .collect()
        };
        for (pane_id, host_session_id) in wanted {
            if stop.is_set() {
                return Ok(());
            }
            match self.0.client.mirror_detail(id, &host_session_id).await {
                Ok(pane) => self.apply_detail(&pane_id, &pane),
                // Closed on the host between its list and this read: the list is read whole next time, which takes it away.
                Err(error) if refusal_status(&error).is_some_and(|s| s != 401) => {
                    let mut book = self.book();
                    if let Some(listed) = book.summary_revisions.get(&pane_id).cloned() {
                        book.detail_revisions.insert(pane_id.clone(), listed);
                    }
                    if let Some(poll) = book.polls.get_mut(id).filter(|p| p.serial == serial) {
                        poll.revision = None;
                    }
                }
                Err(error) => return Err(error),
            }
        }
        Ok(())
    }

    /// `ApplyMirrorDetail(paneId, pane)`: a host pane's detail laid over its pane here, its waiting requests as the pane's cards.
    pub fn apply_detail(&self, pane_id: &str, pane: &MirrorPane) {
        if self.book().closing {
            return;
        }
        let incoming: Vec<ToolPermissionRequest> = pane.session.permissions.iter().filter(|p| p.state == "pending").cloned().collect();
        let now = DateTimeOffset::now();
        let changed = self.0.stage.change(&mut |snapshot| {
            let at = snapshot.sessions().iter().position(|s| s.id() == pane_id && s.mirror.is_some())?;
            let mut next = snapshot.clone();
            let laid = support::apply(&pane.session, &next.sessions()[at], now);
            next.sessions_mut()[at] = Arc::new(laid);
            Some(next)
        });
        if !changed {
            return;
        }
        let (withdrawn, added) = {
            let mut book = self.book();
            let held = book.requests.get(pane_id).cloned().unwrap_or_default();
            let added: Vec<ToolPermissionRequest> = incoming
                .iter()
                .filter(|p| held.iter().all(|(h, _)| h.id != p.id))
                .map(|p| ToolPermissionRequest { run_id: pane_id.into(), ..p.clone() })
                .collect();
            let withdrawn: Vec<ToolPermissionRequest> = held
                .iter()
                .filter(|(h, _)| incoming.iter().all(|p| p.id != h.id))
                .map(|(h, _)| ToolPermissionRequest { state: "cancelled".into(), ..h.clone() })
                .collect();
            if incoming.is_empty() {
                book.requests.remove(pane_id);
            } else {
                let held =
                    incoming.iter().map(|p| (ToolPermissionRequest { run_id: pane_id.into(), ..p.clone() }, p.run_id.clone())).collect();
                book.requests.insert(pane_id.into(), held);
            }
            let counts = book.counts.get(pane_id).cloned().unwrap_or_default();
            book.counts.insert(
                pane_id.into(),
                MirrorCounts {
                    queued: pane.session.queued.len() as i64,
                    queued_items: Some(pane.session.queued.clone()),
                    queue_actions: pane.session.queue_actions,
                    branch: pane.session.branch.clone(),
                    base: pane.session.base.clone(),
                    ..counts
                },
            );
            book.detail_revisions.insert(pane_id.into(), pane.revision.clone());
            (withdrawn, added)
        };
        for request in withdrawn.into_iter().chain(added) {
            self.0.stage.permission(request);
        }
        self.0.stage.changed();
    }

    /// `ForgetMirrorPanes(panes)`: what this PC held of panes that left: their revisions, counts, and cards (withdrawn).
    fn forget(&self, panes: &[String]) {
        if panes.is_empty() {
            return;
        }
        let mut withdrawn = vec![];
        {
            let mut book = self.book();
            for id in panes {
                book.summary_revisions.remove(id);
                book.detail_revisions.remove(id);
                book.counts.remove(id);
                if let Some(held) = book.requests.remove(id) {
                    withdrawn.extend(held.into_iter().map(|(h, _)| ToolPermissionRequest { state: "cancelled".into(), ..h }));
                }
            }
        }
        for request in withdrawn {
            self.0.stage.permission(request);
        }
    }

    /// `MirrorHostRun(paneId, requestId)`: the host's run id of a mirrored pane's waiting request.
    fn host_run(&self, pane_id: &str, request_id: &str) -> Option<String> {
        self.book().requests.get(pane_id)?.iter().find(|(h, _)| h.id == request_id).map(|(_, run)| run.clone())
    }

    /// `AnswerMirror(paneId, mirror, requestId, answer)`: an answer to a mirrored pane's request goes to its host without holding the
    /// caller; a failure is said ([`MirrorStage::failed`]).
    fn answer(&self, pane_id: String, request_id: String, answer: Answer) {
        let Some(mirror) = self.reference(&pane_id) else { return };
        let host_run = self.host_run(&pane_id, &request_id);
        let desk = self.clone();
        (self.0.spawn)(Box::pin(async move {
            let client = &desk.0.client;
            let (connection, host) = (mirror.connection_id.as_str(), mirror.host_session_id.as_str());
            let result = match host_run {
                None => Err(get("remote.error.notPendingPermission")),
                Some(run) => match answer {
                    Answer::Permission(allow) => client.mirror_permission(connection, host, &request_id, &run, allow, false).await,
                    Answer::Questions(answers) => client.mirror_answers(connection, host, &request_id, &run, answers).await,
                    Answer::Plan(decision) => client.mirror_plan(connection, host, &request_id, &run, &decision).await,
                }
                .map_err(|e| e.message()),
            };
            if let Err(words) = result {
                desk.0.stage.failed(&pane_id, &words);
            }
            desk.wake(connection);
        }));
    }

    /// `MirrorSubmitAsync(paneId, text, attachments)`: the composer's text and files as the host pane's next request (the host's word for
    /// what it did). A pane whose computer is not connected is refused before anything goes; a refusal is the host's words.
    pub async fn submit(&self, pane_id: &str, text: &str, attachments: Vec<RunAttachment>) -> Result<String, String> {
        let mirror = self.reference(pane_id).ok_or_else(|| get("remote.error.paneNotFound"))?;
        if let Some(reason) = self.blocked_reason(&mirror) {
            return Err(reason);
        }
        if text.trim().is_empty() && attachments.is_empty() {
            return Err(get("validation.run.badInput"));
        }
        let outcome = self
            .0
            .client
            .mirror_submit(&mirror.connection_id, &mirror.host_session_id, text, attachments)
            .await
            .map_err(|e| e.message())?;
        self.wake(&mirror.connection_id);
        Ok(outcome)
    }

    /// MC-49: × or 바로 실행 on a queued request of a mirrored pane, taken on its host by the host's own rules (the phone's queue
    /// actions): the host's word for what 바로 실행 did (`here`, `worktree`), `removed` for a removal. A pane whose computer is not
    /// connected, or whose host does not take the queue's actions (an older Mighty Code), is refused before anything goes; a refusal is
    /// the host's words. The row leaves this pane's queue at once, and the poll reads the host again.
    pub async fn queue_action(&self, pane_id: &str, item_id: &str, action: QueueAction) -> Result<String, String> {
        let mirror = self.reference(pane_id).ok_or_else(|| get("remote.error.paneNotFound"))?;
        if let Some(reason) = self.blocked_reason(&mirror) {
            return Err(reason);
        }
        let counts = self.counts(pane_id).unwrap_or_default();
        if !counts.queue_actions {
            return Err(get("remote.mirror.error.unavailable"));
        }
        let (connection, host) = (mirror.connection_id.as_str(), mirror.host_session_id.as_str());
        let done = match action {
            QueueAction::Remove => self.0.client.mirror_queue_remove(connection, host, item_id).await.map(|()| "removed".to_string()),
            QueueAction::RunNow => self.0.client.mirror_queue_run_now(connection, host, item_id).await,
        };
        let done = done.map_err(|e| e.message());
        if done.is_ok() {
            let mut book = self.book();
            if let Some(held) = book.counts.get_mut(pane_id)
                && let Some(items) = held.queued_items.as_mut()
                && let Some(at) = items.iter().position(|q| q.id == item_id)
            {
                items.remove(at);
                held.queued = (held.queued - 1).max(items.len() as i64);
            }
        }
        self.wake(connection);
        if done.is_ok() {
            self.0.stage.changed();
        }
        done
    }

    /// `MirrorStopAsync(mirror)`.
    pub async fn stop_pane(&self, mirror: &MirrorReference) -> Result<(), String> {
        let result = self.0.client.mirror_stop(&mirror.connection_id, &mirror.host_session_id).await.map_err(|e| e.message());
        self.wake(&mirror.connection_id);
        result
    }

    /// `CreateMirrorPaneAsync(workspaceId, kind, provider)`: a new agent pane on that computer, in the shared workspace; its id here once
    /// the next list shows it (None when it did not within five seconds).
    pub async fn create_pane(&self, workspace_id: &str, kind: &str, provider: &str) -> Result<Option<String>, String> {
        let snapshot = self.0.stage.snapshot();
        let remote =
            snapshot.workspace(workspace_id).and_then(mirror_of).cloned().ok_or_else(|| get("remote.workspace.error.notShared"))?;
        if kind != "claude" {
            return Err(get("remote.mirror.error.commandPane"));
        }
        let connection = remote.connection_id.clone().unwrap_or_default();
        let host_workspace = remote.workspace_id.clone().unwrap_or_default();
        let host_id = self.0.client.mirror_create_session(&connection, &host_workspace, provider).await.map_err(|e| e.message())?;
        {
            let mut book = self.book();
            if let Some(poll) = book.polls.get_mut(&connection) {
                poll.revision = None;
                poll.wake.store(true, Ordering::SeqCst);
            }
        }
        let client_id = support::client_id(&connection, &host_id);
        for _ in 0..50 {
            if self.0.stage.snapshot().session(&client_id).is_some() {
                return Ok(Some(client_id));
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        Ok(None)
    }

    /// `CloseMirrorPaneAsync(paneId)`: closes the pane on that computer, as the phone's close does; it leaves here at once.
    pub async fn close_pane(&self, pane_id: &str) -> Result<(), String> {
        let mirror = self.reference(pane_id).ok_or_else(|| get("remote.error.paneNotFound"))?;
        self.0.client.mirror_close(&mirror.connection_id, &mirror.host_session_id).await.map_err(|e| e.message())?;
        self.0.stage.change(&mut |snapshot| {
            let mut next = snapshot.clone();
            next.sessions_mut().retain(|s| s.id() != pane_id);
            if next.active_session_id.as_deref() == Some(pane_id) {
                let active = next.active_workspace_id.clone();
                next.active_session_id =
                    next.sessions().iter().find(|s| Some(s.workspace_id()) == active.as_deref()).map(|s| s.id().to_string());
            }
            Some(next)
        });
        self.forget(&[pane_id.to_string()]);
        self.wake(&mirror.connection_id);
        self.0.stage.changed();
        Ok(())
    }

    /// `ConnectMirrorAsync(connectionId)`: the connection's mirror starts after a connect the user asked for (Settings' 보기 and 연결, the
    /// sidebar server's 연결), a host that lacks it opening as before.
    pub async fn connect(&self, connection_id: &str) -> Result<(), String> {
        self.0.client.refresh(connection_id).await.map_err(|e| e.message())?;
        self.start(connection_id);
        Ok(())
    }

    /// `StopMirrorPolls`: the app quits; every poll ends and no new one starts.
    pub fn dispose(&self) {
        let mut book = self.book();
        book.closing = true;
        for poll in book.polls.values() {
            poll.stop.stop();
        }
    }
}

/// The run service's door for mirrored panes (C# DesktopService.cs's `MirrorReference` checks).
impl MirrorRunner for MirrorDesk {
    fn submit(&self, session_id: String, text: String, attachments: Vec<RunAttachment>) -> BoxFuture<'_, RunResult<String>> {
        Box::pin(async move { MirrorDesk::submit(self, &session_id, &text, attachments).await.map_err(RunError::State) })
    }

    fn stop(&self, session_id: String) -> BoxFuture<'_, RunResult<()>> {
        Box::pin(async move {
            let Some(mirror) = self.reference(&session_id) else { return Ok(()) };
            self.stop_pane(&mirror).await.map_err(RunError::State)
        })
    }

    fn respond_to_tool_permission(&self, session_id: String, request_id: String, allow: bool) {
        self.answer(session_id, request_id, Answer::Permission(allow));
    }

    fn answer_questions(&self, session_id: String, request_id: String, answers: IndexMap<String, UserQuestionAnswer>) {
        self.answer(session_id, request_id, Answer::Questions(answers));
    }

    fn answer_plan(&self, session_id: String, request_id: String, decision: PlanDecision) {
        self.answer(session_id, request_id, Answer::Plan(decision));
    }

    fn looking_at(&self, workspace_id: &str) {
        self.wake_for(workspace_id);
    }
}
