//! The push hub (C# `MobilePushHub`, R\MobilePushHub.cs; plan §1.3): driven by the app's 250 ms tick, it reads the panes only when
//! something moved, bridges a run that started and ended between two reads as `running`, plans (`planner`), picks the recipients
//! (`recipients`), seals (`seal`) and sends (`transport`); nothing goes out while the remote is off. The m1 routes `/m1/push/register`,
//! `unregister` and `viewing` (W8-B's router) reach it. Best effort: failures are traced only, and never with a handle, a token, an
//! envelope or what a notification says. Owner: W8-A.
//!
//! Where C# subscribes to `DesktopService`'s events and keeps its own list of pending requests, the hub here is told by the app
//! ([`PushHub::changed`], [`PushHub::began`]) and reads the panes through a [`PushSource`] (src-tauri `mobile.rs` projects them with
//! [`crate::push::planner::project`] over the run service's pending requests, which already drops a pane's requests once it is gone).

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, RwLock, Weak};
use std::time::Duration;

use mighty_platform::{BoxFuture, HttpsSend};
use tokio::sync::watch;
use tokio::task::JoinHandle;

use crate::identity::{Identity, PushRegistrationResult, RotationSubscription};
use crate::push::message::PushMessage;
use crate::push::planner::{Planner, PushPane};
use crate::push::recipients::{self, VIEWING_WINDOW, Viewing};
use crate::push::seal;
use crate::push::transport::{HttpPushTransport, PushOutcome, PushSendRequest, PushStatus, PushTransport, send_uri};

/// `MobilePushHub.MaximumRetryWait`: the longest single wait a relay's Retry-After can ask for; two retries bound a send to about a
/// minute.
pub const MAXIMUM_RETRY_WAIT: Duration = Duration::from_secs(30);
/// How long a phone's viewing record lasts.
pub const VIEWING: Duration = VIEWING_WINDOW;
/// The waits before the second and third attempt.
pub const RETRY_DELAYS: [Duration; 2] = [Duration::from_secs(2), Duration::from_secs(8)];

/// What the hub reads at an observation (C#'s `MobilePushPane.Project` over `DesktopService`, and `SignInLost`).
pub trait PushSource {
    /// The panes as the planner sees them now.
    fn panes(&self) -> Vec<PushPane>;
    /// Whether the pane's sign-in was confirmed lost.
    fn sign_in_lost(&self, session_id: &str) -> bool;
}

type Wait = Arc<dyn Fn(Duration) -> BoxFuture<'static, ()> + Send + Sync>;

/// The hub's seams (C#'s internal properties): the checks replace them.
#[derive(Clone)]
pub struct PushSeams {
    pub transport: Arc<dyn PushTransport>,
    /// Waits before the second and third attempt; a relay's Retry-After wins when longer, up to [`MAXIMUM_RETRY_WAIT`].
    pub retry_delays: Vec<Duration>,
    pub wait: Wait,
    /// Milliseconds since the epoch, for the views and the messages' time.
    pub clock: Arc<dyn Fn() -> i64 + Send + Sync>,
    /// The relay pushes go through (the relay host's `RelayForPush`).
    pub relay: Arc<dyn Fn() -> String + Send + Sync>,
    /// Whether phone remote access is on (the relay host's applied settings).
    pub remote_on: Arc<dyn Fn() -> bool + Send + Sync>,
}

impl PushSeams {
    /// The app's: pushes over `https`, 2 s and 8 s retries, the real clock and sleep.
    pub fn new(
        https: Arc<dyn HttpsSend>,
        relay: Arc<dyn Fn() -> String + Send + Sync>,
        remote_on: Arc<dyn Fn() -> bool + Send + Sync>,
    ) -> PushSeams {
        PushSeams {
            transport: Arc::new(HttpPushTransport::new(https)),
            retry_delays: RETRY_DELAYS.to_vec(),
            wait: Arc::new(|wait| Box::pin(tokio::time::sleep(wait))),
            clock: Arc::new(|| mighty_core::time::clock_now().timestamp_millis()),
            relay,
            remote_on,
        }
    }
}

struct Planning {
    planner: Planner,
    /// The panes last handed to the planner; None while phone remote access is off, so the next hand-off starts it over.
    last: Option<Vec<PushPane>>,
}

/// C# `MobilePushHub`: which phone wants what, which session each has on screen, and what to send.
pub struct PushHub {
    identity: Arc<Identity>,
    host_name: String,
    seams: RwLock<PushSeams>,
    viewing: Mutex<HashMap<String, Viewing>>,
    /// Panes whose run (or shell command) started since the last read: one that already ended is still pushed as a run.
    started: Mutex<HashSet<String>>,
    planning: Mutex<Planning>,
    /// Set when something a push depends on was reported; the panes are read again at the next observation.
    moved: AtomicBool,
    /// True once disposed: nothing is sent or tried again after that.
    stopping: watch::Sender<bool>,
    rotation: Mutex<Option<RotationSubscription>>,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// `ToString("yyyy-MM-dd'T'HH:mm:ss'Z'")` of a UTC time in milliseconds since the epoch.
pub fn at_text(ms: i64) -> String {
    let seconds = ms.div_euclid(1000);
    let (days, in_day) = (seconds.div_euclid(86_400), seconds.rem_euclid(86_400));
    // Howard Hinnant's civil_from_days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z", in_day / 3600, in_day / 60 % 60, in_day % 60)
}

impl PushHub {
    /// `new MobilePushHub(service, host, hostName)`: subscribed to the identity's rotation (a rotated key forgets every view).
    pub fn new(identity: Arc<Identity>, host_name: &str, seams: PushSeams) -> Arc<PushHub> {
        let hub = Arc::new(PushHub {
            identity: identity.clone(),
            host_name: host_name.into(),
            seams: RwLock::new(seams),
            viewing: Mutex::new(HashMap::new()),
            started: Mutex::new(HashSet::new()),
            planning: Mutex::new(Planning { planner: Planner::new(), last: None }),
            moved: AtomicBool::new(true),
            stopping: watch::channel(false).0,
            rotation: Mutex::new(None),
        });
        let weak: Weak<PushHub> = Arc::downgrade(&hub);
        let subscription = identity.on_rotated(move || {
            if let Some(hub) = weak.upgrade() {
                hub.forget_views();
            }
        });
        *lock(&hub.rotation) = Some(subscription);
        hub
    }

    fn seams(&self) -> PushSeams {
        self.seams.read().unwrap_or_else(|poisoned| poisoned.into_inner()).clone()
    }

    /// Replaces the seams (the checks).
    pub fn set_seams(&self, change: impl FnOnce(&mut PushSeams)) {
        change(&mut self.seams.write().unwrap_or_else(|poisoned| poisoned.into_inner()));
    }

    fn stopped(&self) -> bool {
        *self.stopping.borrow()
    }

    /// `Changed`: something a push may follow moved; the panes are read at the next [`PushHub::observe`].
    pub fn changed(&self) {
        self.moved.store(true, Ordering::SeqCst);
    }

    /// `Began`: a pane's run (or a shell's command) started: if it has ended by the next read, it is still pushed as a finished run.
    pub fn began(&self, session_id: &str) {
        lock(&self.started).insert(session_id.to_string());
        self.changed();
    }

    /// `Register(device, handle, kinds)` (`POST /m1/push/register`).
    pub fn register(&self, device: &str, handle: &str, kinds: &[String]) -> PushRegistrationResult {
        self.identity.set_push(device, handle, kinds)
    }

    /// `Unregister(device)` (`POST /m1/push/unregister`): false when the handle could not be cleared from the file (it is still there,
    /// and so is the view).
    pub fn unregister(&self, device: &str) -> bool {
        if !self.identity.clear_push(device, None) {
            return false;
        }
        lock(&self.viewing).remove(device);
        true
    }

    /// `View(device, sessionId)` (`POST /m1/push/viewing`): the phone shows that session now (None: none), for 45 s.
    pub fn viewing(&self, device: &str, session_id: Option<String>) {
        let mut viewing = lock(&self.viewing);
        match session_id {
            None => {
                viewing.remove(device);
            }
            Some(session_id) => {
                let until_ms = (self.seams().clock)() + VIEWING_WINDOW.as_millis() as i64;
                viewing.insert(device.to_string(), Viewing { session_id, until_ms });
            }
        }
    }

    /// The sessions each phone said is on its screen (C#'s `Viewing`).
    pub fn views(&self) -> HashMap<String, Viewing> {
        lock(&self.viewing).clone()
    }

    /// A rotation or revoke: every viewing record dropped (every phone pairs again, so none of them is looking at anything here).
    pub fn forget_views(&self) {
        lock(&self.viewing).clear();
    }

    /// `Observe` (the 250 ms tick): nothing while phone remote access is off; the panes read again only when something was reported
    /// since the last pass or a pane's sign-in changed; what to push sent off the caller's task (the handles it answers).
    pub fn observe(self: &Arc<Self>, source: &dyn PushSource) -> Vec<JoinHandle<()>> {
        if self.stopped() {
            return vec![];
        }
        if !(self.seams().remote_on)() {
            lock(&self.planning).last = None;
            lock(&self.started).clear();
            self.changed();
            return vec![];
        }
        let previous = lock(&self.planning).last.clone();
        let moved = self.moved.swap(false, Ordering::SeqCst);
        if !moved
            && previous.as_ref().is_some_and(|panes| panes.iter().all(|pane| source.sign_in_lost(&pane.session_id) == pane.sign_in_lost))
        {
            return vec![];
        }
        let begun: HashSet<String> = std::mem::take(&mut *lock(&self.started));
        let panes = source.panes();
        let Some(previous) = previous.filter(|_| !begun.is_empty()) else { return self.push(&panes) };
        // A run that started and ended between two reads: handed on as running first, so the planner sees it end (the Mac sees every
        // change). Both hand-offs are this one read's, in order.
        let before: HashMap<&str, &str> = previous.iter().map(|p| (p.session_id.as_str(), p.status.as_str())).collect();
        let bridged: Vec<PushPane> = panes
            .iter()
            .map(|pane| {
                if begun.contains(&pane.session_id) && pane.status != "running" && before.get(pane.session_id.as_str()) != Some(&"running")
                {
                    PushPane { status: "running".into(), ..pane.clone() }
                } else {
                    pane.clone()
                }
            })
            .collect();
        if bridged == panes {
            return self.push(&panes);
        }
        let mut handles = self.push(&bridged);
        handles.extend(self.push(&panes));
        handles
    }

    /// `Push`: one hand-off — the first after phone remote access came on starts the planner over (what changed meanwhile is not
    /// pushed), the same panes again decide nothing, and what the planner decides is sealed per phone and sent off the caller's task.
    pub fn push(self: &Arc<Self>, panes: &[PushPane]) -> Vec<JoinHandle<()>> {
        let seams = self.seams();
        let now = (seams.clock)();
        let messages = {
            let mut planning = lock(&self.planning);
            // Off or shut down, nothing goes out whatever the caller hands on.
            if self.stopped() || !(seams.remote_on)() {
                planning.last = None;
                return vec![];
            }
            if planning.last.as_deref() == Some(panes) {
                return vec![];
            }
            if planning.last.is_none() {
                planning.planner.reset();
            }
            planning.last = Some(panes.to_vec());
            planning.planner.observe(panes, &self.identity.host_id(), &self.host_name, &at_text(now))
        };
        if messages.is_empty() {
            return vec![];
        }
        let hub = self.clone();
        vec![tokio::spawn(async move { hub.send(messages, now, seams).await })]
    }

    async fn send(self: Arc<Self>, messages: Vec<PushMessage>, now: i64, seams: PushSeams) {
        let devices = self.identity.push_devices();
        if devices.is_empty() {
            return;
        }
        let Some(url) = send_uri(&(seams.relay)()) else {
            eprintln!("mobile push: no relay to send through");
            return;
        };
        let host_token = self.identity.host_token();
        let server_id = self.identity.server_id();
        let seen = {
            let mut viewing = lock(&self.viewing);
            viewing.retain(|_, view| view.until_ms > now);
            viewing.clone()
        };
        let mut deliveries = tokio::task::JoinSet::new();
        for message in &messages {
            let Some(plaintext) = message.encoded_for_push() else {
                eprintln!("mobile push: {} is too large even without its summary; skipped", message.id);
                continue;
            };
            for device in recipients::targets(message, &devices, &seen, now) {
                let Some(key) = seal::key(&device.token_hash, &device.device_id) else {
                    eprintln!("mobile push: {} could not be sealed for a phone; skipped", message.id);
                    continue;
                };
                let Ok(payload) = seal::seal(&plaintext, &key, &device.handle, None) else { continue };
                let request = PushSendRequest { server_id: server_id.clone(), handle: device.handle.clone(), payload };
                let (hub, url, token, device_id, message_id, seams) =
                    (self.clone(), url.clone(), host_token.to_string(), device.device_id.clone(), message.id.clone(), seams.clone());
                deliveries.spawn(async move { hub.deliver(request, url, token, device_id, message_id, seams).await });
            }
        }
        while deliveries.join_next().await.is_some() {}
    }

    /// One phone's delivery: two retries (2 s, 8 s; a longer Retry-After wins up to 30 s), an exception a retry, a refusal the end,
    /// "gone" forgetting only the handle sent to, and switching off or closing stopping it without a retry.
    async fn deliver(
        &self,
        request: PushSendRequest,
        url: String,
        host_token: String,
        device_id: String,
        message_id: String,
        seams: PushSeams,
    ) {
        let mut waits: std::collections::VecDeque<Duration> = seams.retry_delays.iter().copied().collect();
        let mut stopping = self.stopping.subscribe();
        loop {
            // Switched off or closing meanwhile: nothing more goes out.
            if self.stopped() || !(seams.remote_on)() {
                return;
            }
            let send = seams.transport.send(request.clone(), url.clone(), host_token.clone());
            let outcome: PushOutcome = tokio::select! {
                outcome = send => outcome.unwrap_or_else(|error| {
                    eprintln!("mobile push: {message_id} {}", crate::identity::error_kind(&error));
                    PushOutcome::new(PushStatus::Retry)
                }),
                _ = until_true(&mut stopping) => return,
            };
            match outcome.status {
                PushStatus::Sent => return,
                PushStatus::Gone => {
                    // Only the handle that was sent to: one the phone registered since stays.
                    if self.identity.clear_push(&device_id, Some(&request.handle)) {
                        eprintln!("mobile push: {message_id} the relay no longer knows a phone's handle; dropped it");
                    } else {
                        eprintln!("mobile push: {message_id} the relay no longer knows a phone's handle, which could not be forgotten");
                    }
                    return;
                }
                PushStatus::Failed => {
                    eprintln!("mobile push: {message_id} refused by the relay ({})", outcome.code);
                    return;
                }
                PushStatus::Retry => {
                    let Some(mut wait) = waits.pop_front() else {
                        eprintln!("mobile push: {message_id} the relay could not take it; gave up");
                        return;
                    };
                    if self.stopped() || !(seams.remote_on)() {
                        return;
                    }
                    if let Some(asked) = outcome.retry_after.filter(|asked| *asked > wait) {
                        wait = asked;
                    }
                    wait = wait.min(MAXIMUM_RETRY_WAIT);
                    if wait.is_zero() {
                        continue;
                    }
                    tokio::select! {
                        _ = (seams.wait)(wait) => {}
                        _ = until_true(&mut stopping) => return,
                    }
                }
            }
        }
    }

    /// `Dispose`: nothing is planned, sent or tried again from here on; a send under way is cancelled. Idempotent.
    pub fn dispose(&self) {
        self.stopping.send_replace(true);
        lock(&self.rotation).take();
    }
}

/// Waits until the flag reads true (or its sender is gone); the borrow it takes ends before this returns, so a `select!` over it stays
/// `Send`.
async fn until_true(flag: &mut watch::Receiver<bool>) {
    let _ = flag.wait_for(|set| *set).await;
}
