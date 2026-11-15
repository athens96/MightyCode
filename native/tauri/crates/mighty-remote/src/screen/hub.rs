//! Screen share's hub (C# R\ScreenShareHub.cs; plan §1.8): host-authoritative consent (the grants in `screen-devices.json`), the P-256
//! control-key enrollment with the host's confirmation and the one-use challenge, the E2EE routes (`state`, `control-key`,
//! `sessions`), the `screen-*` signalling frames, the session limits (1 control, 2 view; one capture serves every phone), the input
//! policy on the `screen-control` channel, the displays that come and go, the watcher (lock and password focus at once, 30 s in the
//! background or unconnected, 600/1800 s idle) and the kill reasons. Owner: W8-F.
//!
//! No input contents, SDP, ICE passwords or clipboard bytes enter a log, an error or `Debug`.
//!
//! The hub drives a [`ScreenSharePlatform`]: the capture, the input and the WebRTC peer of C#'s `IScreenSharePlatform`. Screen share
//! is switched off ([`super::ENABLED`]), so the app builds no platform and no hub (C# `InitializeScreenShare` returns at once); the
//! native platform over mighty-platform's `ScreenCapture` and `RemoteInput` ports and the hidden-WebView2 transport come after 0.5
//! (D11). The checks drive the hub over a fake platform.

use std::collections::HashMap;
use std::future::Future;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use chrono::{DateTime, TimeDelta, Utc};
use indexmap::IndexMap;
use mighty_core::json::{self, JsonOut, Wire as _};
use mighty_platform::{BoxFuture, Result};
use serde_json::Value;
use tokio::task::{JoinHandle, JoinSet};
use tokio::time::timeout;
use tokio_util::sync::CancellationToken;

use super::clipboard::ScreenClipboard;
use super::policy::{
    self, ScreenDeviceGrant, ScreenDisplay, ScreenLiveSession, ScreenQuality, ScreenRegion, StoredGrant, client_id, from_base64, to_base64,
};
use super::scene;
use super::wire::{self, ScreenIceServer, int32};
use crate::relay::connection::MobileReply;

/// C# `ScreenSafety(Locked, SecureInput, LastLocalInput)`: whether the PC is locked, whether a secure desktop or a password field has
/// the focus, and when its own keyboard or mouse was last used.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScreenSafety {
    pub locked: bool,
    pub secure_input: bool,
    pub last_local_input: DateTime<Utc>,
}

/// C# `IScreenSharePlatform`: the capture, input and peer the hub drives.
///
/// Contract: `safety` never fails — a probe that cannot tell reports `secure_input` (fail closed); `displays` lists what is attached
/// now; `set_display` answers false when that display is not attached (any longer); a future may be dropped part way (the hub's
/// cancellation), and must leave no input held. Nothing here is called while screen share is off.
pub trait ScreenSharePlatform: Send + Sync {
    fn available(&self) -> bool;
    fn displays(&self) -> Vec<ScreenDisplay>;
    fn safety(&self) -> ScreenSafety;
    fn start_peer(&self, session: ScreenLiveSession, ice: Vec<ScreenIceServer>) -> BoxFuture<'_, Result<()>>;
    fn receive_signal(&self, session_id: String, signal: Value) -> BoxFuture<'_, Result<()>>;
    fn stop_peer(&self, session_id: String) -> BoxFuture<'_, Result<()>>;
    /// Closes the pixel gate at once (every peer's frames stop).
    fn halt(&self) -> Result<()>;
    fn set_display(&self, display_id: i32) -> BoxFuture<'_, Result<bool>>;
    fn set_region(&self, region: ScreenRegion);
    fn inject(&self, session_id: String, input: Value) -> BoxFuture<'_, Result<()>>;
    fn release_buttons(&self, session_id: String) -> BoxFuture<'_, Result<()>>;
    fn show_marker(&self, session_id: String, input: Value) -> BoxFuture<'_, Result<bool>>;
    /// None for a concealed source (a password manager's copy).
    fn read_clipboard(&self) -> BoxFuture<'_, Result<Option<String>>>;
    fn write_clipboard(&self, text: String) -> BoxFuture<'_, Result<()>>;
    /// One data-channel message (JSON text) to the session's phone.
    fn send_data(&self, session_id: String, message: String) -> BoxFuture<'_, Result<()>>;
}

/// C# `send`: a relay frame (JSON text) to every connection of a device (`MobileRelayHost.SendToDeviceAsync`).
pub type DeviceSend = Arc<dyn Fn(String, String) -> BoxFuture<'static, Result<()>> + Send + Sync>;
/// C# `confirm`: asks the user whether a device's new control key (its fingerprint) may control this PC. Dropping the future cancels
/// the question.
pub type ConfirmKey = Arc<dyn Fn(String, String) -> BoxFuture<'static, bool> + Send + Sync>;
/// C# `trustedDevice`: whether a device is paired now (`MobileRelayHost.IsTrustedDevice`).
pub type TrustedDevice = Arc<dyn Fn(&str) -> bool + Send + Sync>;
/// C# `now`.
pub type Clock = Arc<dyn Fn() -> DateTime<Utc> + Send + Sync>;
type Changed = Arc<dyn Fn() + Send + Sync>;

struct Session {
    value: ScreenLiveSession,
    cancel: CancellationToken,
    activity: DateTime<Utc>,
    background: Option<DateTime<Utc>>,
    connected: bool,
    clipboard: Arc<Mutex<ScreenClipboard>>,
}

struct Challenge {
    bytes: Vec<u8>,
    expires: DateTime<Utc>,
}

#[derive(Default)]
struct State {
    grants: IndexMap<String, ScreenDeviceGrant>,
    sessions: IndexMap<String, Session>,
    challenges: HashMap<String, Challenge>,
    refused_keys: HashMap<String, DateTime<Utc>>,
    /// When capture was last started again after it ended on its own (`policy::may_restart_capture`).
    restarts: Vec<DateTime<Utc>>,
    ice: Vec<ScreenIceServer>,
    confirming: bool,
    disposed: bool,
}

struct Inner {
    path: PathBuf,
    platform: Arc<dyn ScreenSharePlatform>,
    send: DeviceSend,
    confirm: ConfirmKey,
    trusted: TrustedDevice,
    now: Clock,
    state: Mutex<State>,
    changed: Mutex<(u64, Vec<(u64, Changed)>)>,
    lifetime: CancellationToken,
}

/// A route's refusal (C# `ScreenShareException` and the exceptions `RouteAsync` maps to 400), or a failed save (which C# lets out).
enum Refusal {
    Reason(&'static str, u16),
    Io(io::Error),
}

fn refuse<T>(reason: &'static str) -> std::result::Result<T, Refusal> {
    Err(Refusal::Reason(reason, 403))
}

fn bad_request<T>() -> std::result::Result<T, Refusal> {
    Err(Refusal::Reason("bad-request", 400))
}

/// C# `ScreenShareHub`.
pub struct ScreenShareHub {
    inner: Arc<Inner>,
    watcher: Mutex<Option<JoinHandle<()>>>,
}

impl ScreenShareHub {
    /// Reads `screen-devices.json` in `directory` (made if missing; a linked folder, a linked file or one over 64 KiB is refused; a
    /// file that does not read gives no grants) and starts the watcher. Runs inside a tokio runtime. Call [`dispose`](Self::dispose)
    /// before dropping it: a dropped hub stops its watcher, but ends no session.
    pub fn new(
        directory: &Path,
        platform: Arc<dyn ScreenSharePlatform>,
        send: DeviceSend,
        confirm: ConfirmKey,
        trusted: TrustedDevice,
        now: Option<Clock>,
    ) -> io::Result<ScreenShareHub> {
        secure_directory(directory)?;
        let path = directory.join("screen-devices.json");
        let grants = load(&path)?;
        let inner = Arc::new(Inner {
            path,
            platform,
            send,
            confirm,
            trusted,
            now: now.unwrap_or_else(|| Arc::new(Utc::now)),
            state: Mutex::new(State { grants, ..State::default() }),
            changed: Mutex::new((0, Vec::new())),
            lifetime: CancellationToken::new(),
        });
        let watcher = tokio::spawn(inner.clone().watch());
        Ok(ScreenShareHub { inner, watcher: Mutex::new(Some(watcher)) })
    }

    /// `InputPaused`: a control key waits for the user's answer.
    pub fn input_paused(&self) -> bool {
        self.inner.lock().confirming
    }

    pub fn ice_servers(&self) -> Vec<ScreenIceServer> {
        self.inner.lock().ice.clone()
    }

    pub fn sessions(&self) -> Vec<ScreenLiveSession> {
        self.inner.lock().sessions.values().map(|s| s.value.clone()).collect()
    }

    pub fn grants(&self) -> Vec<ScreenDeviceGrant> {
        self.inner.lock().grants.values().cloned().collect()
    }

    /// `Changed +=`: called after any session or grant changed (never with the hub's lock held). The id takes it off again.
    pub fn on_changed(&self, callback: Arc<dyn Fn() + Send + Sync>) -> u64 {
        let mut changed = self.inner.changed.lock().unwrap_or_else(PoisonError::into_inner);
        changed.0 += 1;
        let id = changed.0;
        changed.1.push((id, callback));
        id
    }

    /// `Changed -=`.
    pub fn remove_changed(&self, id: u64) {
        self.inner.changed.lock().unwrap_or_else(PoisonError::into_inner).1.retain(|(each, _)| *each != id);
    }

    /// `SetGrantAsync`: a paired device's grant, saved; its sessions the new grant no longer allows end (`grant-downgrade`, or
    /// `revoked` when it is no longer allowed), and the device is told. An unpaired device, a bad id or grant is `InvalidInput`.
    pub async fn set_grant(&self, device: &str, allowed: bool, grant: &str) -> io::Result<()> {
        let (device, grant) = (device.to_string(), grant.to_string());
        self.run(move |inner| async move { inner.set_grant(&device, allowed, &grant).await }).await
    }

    /// `RemoveControlKeyAsync`: the grant stays, its control key goes (enrolled again on the PC), and its control sessions end.
    pub async fn remove_control_key(&self, device: &str) -> io::Result<()> {
        let device = device.to_string();
        self.run(move |inner| async move { inner.remove_control_key(&device).await }).await
    }

    /// `ResetAsync`: every grant, challenge and refusal forgotten (a pairing rotation), every session killed with `reason`.
    pub async fn reset(&self, reason: &str) -> io::Result<()> {
        let reason = reason.to_string();
        self.run(move |inner| async move {
            {
                let mut state = inner.lock();
                inner.save(&mut state, IndexMap::new())?;
                state.challenges.clear();
                state.refused_keys.clear();
            }
            inner.end_all(&reason, true).await;
            Ok(())
        })
        .await
    }

    /// `RouteAsync`: `/m1/screen-share/<leaf>` for an authenticated device (`body` is the request body's JSON text). A failed save
    /// is the error (C# lets its `IOException` out to the router's 500); every other failure is a reply.
    pub async fn route(&self, method: &str, leaf: &str, body: Option<&str>, device: &str) -> io::Result<MobileReply> {
        match self.inner.route(method, leaf, body, device).await {
            Ok(reply) => Ok(reply),
            Err(Refusal::Reason(reason, status)) => Ok(MobileReply::new(status, wire::error(reason))),
            Err(Refusal::Io(error)) => Err(error),
        }
    }

    /// `SignalAsync`: a phone's `screen-*` relay frame (its JSON text) for one of its own sessions.
    pub async fn signal(&self, device: &str, signal: &str) -> Result<()> {
        let (device, signal) = (device.to_string(), signal.to_string());
        self.run(move |inner| async move { inner.signal(&device, &signal).await }).await
    }

    /// `PeerFailedAsync`.
    pub async fn peer_failed(&self, session_id: &str) {
        let id = session_id.to_string();
        self.run(move |inner| async move { inner.stop(&id, "peer-left", false).await }).await
    }

    /// `PeerConnected`.
    pub fn peer_connected(&self, session_id: &str) {
        let now = (self.inner.now)();
        if let Some(session) = self.inner.lock().sessions.get_mut(session_id) {
            session.connected = true;
            session.activity = now;
        }
    }

    /// `PeerSignal`: the peer's offer or candidate (JSON text) to the session's phone.
    pub async fn peer_signal(&self, session_id: &str, signal: String) -> Result<()> {
        let device = self.inner.lock().sessions.get(session_id).map(|s| s.value.device_id.clone());
        match device {
            Some(device) => (self.inner.send)(device, signal).await,
            None => Ok(()),
        }
    }

    /// `InputAsync`: one `screen-control` frame (its text) from a session's phone.
    pub async fn input(&self, session_id: &str, input: &str) {
        self.inner.input(session_id, input).await;
    }

    /// `DeviceDisconnected`: the device's sessions end (`peer-left`).
    pub async fn device_disconnected(&self, device: &str) {
        let device = device.to_string();
        self.run(move |inner| async move {
            let ids: Vec<String> =
                inner.lock().sessions.values().filter(|s| s.value.device_id == device).map(|s| s.value.session_id.clone()).collect();
            for id in ids {
                inner.stop(&id, "peer-left", false).await;
            }
        })
        .await
    }

    /// `StopSessionAsync`: the PC's per-session Stop (`user-stop`); a stale one does nothing.
    pub async fn stop_session(&self, session_id: &str) {
        let id = session_id.to_string();
        self.run(move |inner| async move { inner.stop(&id, "user-stop", false).await }).await
    }

    /// `KillAllAsync`: every session killed with `reason` (`kill-switch` from the stop control).
    pub async fn kill_all(&self, reason: &str) {
        let reason = reason.to_string();
        self.run(move |inner| async move { inner.end_all(&reason, true).await }).await
    }

    /// `DisplayLostAsync`: the capture ended on its own (the display was unplugged, or the OS took the stream). The viewers move to
    /// the main display instead of being stopped and each phone is told the displays there are now; with no display left, when
    /// capture cannot start there, or when it keeps ending (`policy::may_restart_capture`), the sessions end as `display-gone`. A
    /// locked PC or a password field kills them at once with that reason, as the watcher would: capture has already stopped.
    pub async fn display_lost(&self) {
        self.run(|inner| async move { inner.display_lost().await }).await
    }

    /// `CaptureFailedAsync`: a frame that could not be read or encoded: capture is not started again, the sessions end as
    /// `display-gone`.
    pub async fn capture_failed(&self) {
        self.run(|inner| async move {
            let any = !inner.lock().sessions.is_empty();
            if any {
                inner.end_all("display-gone", false).await;
            }
        })
        .await
    }

    /// `BroadcastSceneAsync`: the reference scene's phase to every session's phone; another word is `InvalidInput`.
    pub async fn broadcast_scene(&self, phase: &str) -> io::Result<()> {
        let Some(announcement) = scene::announcement(phase) else {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "Unknown measurement phase."));
        };
        let mut out = JsonOut::new();
        out.begin_object();
        for (name, value) in announcement {
            out.prop_str(name, Some(value));
        }
        out.end_object();
        let message = out.finish();
        self.run(move |inner| async move {
            let mut each = JoinSet::new();
            for session in inner.sessions() {
                let (inner, message) = (inner.clone(), message.clone());
                each.spawn(async move {
                    let send = inner.platform.send_data(session.session_id, message);
                    tokio::select! {
                        _ = inner.lifetime.cancelled() => {}
                        _ = timeout(Duration::from_secs(2), send) => {}
                    }
                });
            }
            each.join_all().await;
        })
        .await;
        Ok(())
    }

    /// `SetIceServersAsync`: the relay's TURN credentials; each live session is sent them and its peer restarts ICE, or it ends.
    pub async fn set_ice_servers(&self, servers: Vec<ScreenIceServer>) {
        self.run(move |inner| async move { inner.set_ice_servers(servers).await }).await
    }

    /// `DisposeAsync`: no grant changes after this, the watcher stops, every session is killed (`kill-switch`).
    pub async fn dispose(&self) {
        self.inner.lock().disposed = true;
        self.inner.lifetime.cancel();
        self.kill_all("kill-switch").await;
        let watcher = self.watcher.lock().unwrap_or_else(PoisonError::into_inner).take();
        if let Some(watcher) = watcher {
            let _ = watcher.await;
        }
    }

    /// Runs a step on a task of its own and waits for it: C# runs these to their end, so a caller that stops waiting leaves no
    /// half-done teardown.
    async fn run<F, T>(&self, step: impl FnOnce(Arc<Inner>) -> F) -> T
    where
        F: Future<Output = T> + Send + 'static,
        T: Send + 'static,
    {
        match tokio::spawn(step(self.inner.clone())).await {
            Ok(value) => value,
            Err(error) if error.is_panic() => std::panic::resume_unwind(error.into_panic()),
            // The runtime is going away: nothing is left to wait for.
            Err(_) => std::future::pending().await,
        }
    }
}

impl Drop for ScreenShareHub {
    fn drop(&mut self) {
        self.inner.lifetime.cancel();
    }
}

impl std::fmt::Debug for ScreenShareHub {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let state = self.inner.lock();
        f.debug_struct("ScreenShareHub")
            .field("sessions", &state.sessions.len())
            .field("grants", &state.grants.len())
            .finish_non_exhaustive()
    }
}

impl Inner {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn sessions(&self) -> Vec<ScreenLiveSession> {
        self.lock().sessions.values().map(|s| s.value.clone()).collect()
    }

    fn changed(&self) {
        let callbacks: Vec<Changed> =
            self.changed.lock().unwrap_or_else(PoisonError::into_inner).1.iter().map(|(_, callback)| callback.clone()).collect();
        for callback in callbacks {
            callback();
        }
    }

    /// `Save`: the grants written whole to a new file beside the old one (owner-only on Unix), flushed, then moved over it; only then
    /// are they the hub's.
    fn save(&self, state: &mut State, next: IndexMap<String, ScreenDeviceGrant>) -> io::Result<()> {
        let mut out = JsonOut::new();
        out.begin_array();
        for grant in next.values() {
            StoredGrant::from(grant).write(&mut out);
        }
        out.end_array();
        let mut temporary = self.path.clone().into_os_string();
        temporary.push(format!(".{}", uuid::Uuid::new_v4().simple()));
        let temporary = PathBuf::from(temporary);
        let written = (|| {
            use std::io::Write as _;
            let mut options = std::fs::OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt as _;
                options.mode(0o600);
            }
            let mut file = options.open(&temporary)?;
            file.write_all(out.as_str().as_bytes())?;
            file.sync_all()?;
            drop(file);
            std::fs::rename(&temporary, &self.path)
        })();
        if temporary.exists() {
            let _ = std::fs::remove_file(&temporary);
        }
        written?;
        state.grants = next;
        Ok(())
    }

    async fn set_grant(self: &Arc<Self>, device: &str, allowed: bool, grant: &str) -> io::Result<()> {
        let stopped: Vec<String> = {
            let mut state = self.lock();
            if state.disposed {
                return Err(io::Error::other("The screen share hub is disposed."));
            }
            if !(self.trusted)(device) || !client_id(device) || !matches!(grant, "none" | "view" | "control") {
                return Err(io::Error::new(io::ErrorKind::InvalidInput, "Invalid device grant."));
            }
            let prior = state.grants.get(device);
            let next = ScreenDeviceGrant {
                device_id: device.to_string(),
                allowed,
                grant: if allowed { grant } else { "none" }.to_string(),
                control_key_public: if allowed && grant == "control" && prior.is_some_and(|p| p.grant == "control") {
                    prior.and_then(|p| p.control_key_public.clone())
                } else {
                    None
                },
            };
            let mut copy = state.grants.clone();
            copy.insert(device.to_string(), next.clone());
            self.save(&mut state, copy)?;
            state.challenges.remove(device);
            state
                .sessions
                .values()
                .filter(|s| s.value.device_id == device && !policy::grant_allows(Some(&next), &s.value.mode))
                .map(|s| s.value.session_id.clone())
                .collect()
        };
        for id in stopped {
            self.stop(&id, if allowed { "grant-downgrade" } else { "revoked" }, true).await;
        }
        let message = {
            let ice = self.lock().ice.clone();
            wire::grant(allowed, if allowed { grant } else { "none" }, &self.platform.displays(), &ice)
        };
        let _ = timeout(Duration::from_secs(1), (self.send)(device.to_string(), message)).await;
        self.changed();
        Ok(())
    }

    async fn remove_control_key(self: &Arc<Self>, device: &str) -> io::Result<()> {
        let ids: Vec<String> = {
            let mut state = self.lock();
            let Some(prior) = state.grants.get(device) else { return Ok(()) };
            let mut copy = state.grants.clone();
            copy.insert(device.to_string(), ScreenDeviceGrant { control_key_public: None, ..prior.clone() });
            self.save(&mut state, copy)?;
            state.challenges.remove(device);
            state
                .sessions
                .values()
                .filter(|s| s.value.device_id == device && s.value.mode == "control")
                .map(|s| s.value.session_id.clone())
                .collect()
        };
        for id in ids {
            self.stop(&id, "revoked", true).await;
        }
        self.changed();
        Ok(())
    }

    /// `Require`: the device's grant when it allows `mode`.
    fn require(&self, state: &State, device: &str, mode: &str) -> std::result::Result<ScreenDeviceGrant, Refusal> {
        if device == "legacy" || !client_id(device) {
            return refuse("legacy-client");
        }
        let Some(grant) = state.grants.get(device).filter(|g| (self.trusted)(device) && g.allowed) else {
            return refuse("device-not-allowed");
        };
        if !policy::grant_allows(Some(grant), mode) {
            return refuse("insufficient-grant");
        }
        Ok(grant.clone())
    }

    /// `ChallengeFor(device, fresh: true)`: a new one-use challenge for a device allowed control, good for two minutes.
    fn challenge_for(&self, state: &mut State, device: &str) -> Option<String> {
        if !state.grants.get(device).is_some_and(|g| g.allowed && g.grant == "control") {
            return None;
        }
        let mut random = [0u8; 24];
        getrandom::fill(&mut random).expect("the OS gives random bytes");
        let bytes = format!("screen-control-challenge:{}:{}", mighty_core::wire::new_id(), to_base64(&random)).into_bytes();
        let text = to_base64(&bytes);
        state.challenges.insert(device.to_string(), Challenge { bytes, expires: (self.now)() + TimeDelta::minutes(2) });
        Some(text)
    }

    async fn route(
        self: &Arc<Self>,
        method: &str,
        leaf: &str,
        body: Option<&str>,
        device: &str,
    ) -> std::result::Result<MobileReply, Refusal> {
        if !self.platform.available() {
            return Err(Refusal::Reason("screen-permission", 503));
        }
        if method == "GET" && leaf == "state" {
            return self.state_reply(device);
        }
        let Some(raw) = body.filter(|raw| raw.len() <= 65536) else { return bad_request() };
        let value: Value = match serde_json::from_str(raw) {
            Ok(value @ Value::Object(_)) => value,
            _ => return bad_request(),
        };
        if method == "POST" && leaf == "control-key" {
            return self.enroll(device, &value).await;
        }
        if method == "POST" && leaf == "sessions" {
            return self.start_session(device, &value);
        }
        Err(Refusal::Reason("not-found", 404))
    }

    fn state_reply(&self, device: &str) -> std::result::Result<MobileReply, Refusal> {
        let mut state = self.lock();
        let grant = if (self.trusted)(device) { state.grants.get(device).cloned() } else { None };
        let key = match grant.as_ref().and_then(|g| g.control_key_public.as_deref()) {
            Some(text) => Some(from_base64(text).map_or_else(bad_request, Ok)?),
            None => None,
        };
        let challenge = self.challenge_for(&mut state, device);
        let mut out = JsonOut::new();
        out.begin_object();
        out.name("screenShare");
        out.begin_object();
        out.prop_bool("allowed", grant.as_ref().is_some_and(|g| g.allowed));
        out.prop_str("grant", Some(grant.as_ref().map_or("none", |g| g.grant.as_str())));
        out.prop_bool("isBeta", true);
        out.name("displays");
        wire::write_displays(&mut out, &self.platform.displays());
        out.prop_str("controlChallengeB64", challenge.as_deref());
        out.prop_str("controlKeyFingerprint", key.as_deref().map(policy::fingerprint).as_deref());
        out.name("iceServers");
        wire::write_ice(&mut out, &state.ice);
        let control = grant.as_ref().is_some_and(|g| g.grant == "control");
        out.prop_int("idleTimeoutSeconds", if control { policy::CONTROL_IDLE_SECONDS } else { policy::VIEW_IDLE_SECONDS });
        out.prop_bool("tapMarker", true);
        out.end_object();
        out.end_object();
        Ok(MobileReply::new(200, out.finish()))
    }

    fn start_session(self: &Arc<Self>, device: &str, value: &Value) -> std::result::Result<MobileReply, Refusal> {
        let live = {
            let mut state = self.lock();
            let mode = json::text(value, "mode").filter(|m| matches!(*m, "view" | "control"));
            let network = json::text(value, "network").filter(|n| matches!(*n, "wifi" | "cellular"));
            let (Some(mode), Some(network), Some(display_id)) = (mode, network, int32(value, "displayId")) else {
                return bad_request();
            };
            let grant = self.require(&state, device, mode)?;
            let safety = self.platform.safety();
            if safety.locked || safety.secure_input || state.confirming {
                return refuse(if safety.locked { "lock-screen" } else { "secure-input" });
            }
            if mode == "control" {
                let challenge = state.challenges.remove(device);
                let now = (self.now)();
                let ready = challenge
                    .filter(|c| c.expires >= now)
                    .zip(grant.control_key_public.as_deref())
                    .zip(json::text(value, "controlSignatureB64"));
                let Some(((challenge, key), signature)) = ready else { return refuse("control-signature") };
                let (Some(signature), Some(key)) = (from_base64(signature), from_base64(key)) else { return bad_request() };
                if !policy::verify(&challenge.bytes, &signature, &key) {
                    return refuse("control-signature");
                }
            }
            let joined = state.sessions.values().map(|s| &s.value);
            if !policy::can_join(mode, joined) || state.sessions.values().any(|s| s.value.device_id == device) {
                return refuse("concurrency-limit");
            }
            let displays = self.platform.displays();
            let Some(mut display) = policy::display_for(&displays, display_id) else { return refuse("screen-permission") };
            // One capture serves every phone: a phone that joins sees the display already captured (M/MightyCore/Remote/
            // ScreenShareEngine.swift:457).
            if let Some(first) = state.sessions.values().next()
                && first.value.display_id != display.display_id
            {
                display = policy::display_for(&displays, first.value.display_id).unwrap_or(display);
            }
            let decodes: Vec<&str> = match value.get("decodes") {
                Some(Value::Array(codecs)) => codecs.iter().filter_map(Value::as_str).collect(),
                _ => vec!["H264"],
            };
            if !decodes.contains(&"H264") {
                return bad_request();
            }
            let now = (self.now)();
            let live = ScreenLiveSession {
                session_id: mighty_core::wire::new_id(),
                device_id: device.to_string(),
                mode: mode.to_string(),
                display_id: display.display_id,
                codec: "H264".into(),
                quality: ScreenQuality::for_network(network == "cellular", false),
                started_at: now,
            };
            state.sessions.insert(
                live.session_id.clone(),
                Session {
                    value: live.clone(),
                    cancel: CancellationToken::new(),
                    activity: now,
                    background: None,
                    connected: false,
                    clipboard: Arc::default(),
                },
            );
            live
        };
        self.changed();
        let mut out = JsonOut::new();
        out.begin_object();
        out.prop_str("sessionId", Some(&live.session_id));
        out.prop_str("mode", Some(&live.mode));
        out.prop_int("displayId", live.display_id.into());
        out.prop_str("codec", Some(&live.codec));
        out.name("quality");
        live.quality.write(&mut out);
        out.end_object();
        let inner = self.clone();
        let id = live.session_id;
        let mut reply = MobileReply::new(200, out.finish());
        // The peer's offer waits until this reply was sent, so the phone knows the session before its first signal.
        // On a task of its own, so a connection that stops waiting leaves no half-stopped session.
        reply.after_send = Some(Box::new(move || {
            let started = tokio::spawn(async move {
                let started = {
                    let state = inner.lock();
                    state.sessions.get(&id).map(|s| (s.value.clone(), s.cancel.clone(), state.ice.clone()))
                };
                let Some((value, cancel, ice)) = started else { return };
                let ok = tokio::select! {
                    _ = cancel.cancelled() => false,
                    started = inner.platform.start_peer(value, ice) => started.is_ok(),
                };
                if !ok {
                    inner.stop(&id, "peer-left", false).await;
                }
            });
            Box::pin(async move {
                let _ = started.await;
            })
        }));
        Ok(reply)
    }

    async fn enroll(self: &Arc<Self>, device: &str, body: &Value) -> std::result::Result<MobileReply, Refusal> {
        let Some(key) = from_base64(json::text(body, "publicKeyB64").unwrap_or("")) else { return bad_request() };
        if !policy::valid_key(&key) {
            return bad_request();
        }
        let fingerprint = policy::fingerprint(&key);
        let before = {
            let mut state = self.lock();
            let before = self.require(&state, device, "control")?;
            if let Some(prior) = before.control_key_public.as_deref() {
                let Some(prior) = from_base64(prior) else { return bad_request() };
                if prior != key {
                    return Err(Refusal::Reason("control-key-present", 409));
                }
                return Ok(MobileReply::new(200, wire::fingerprint(&fingerprint)));
            }
            if state.confirming {
                return Err(Refusal::Reason("control-key-pending", 409));
            }
            let now = (self.now)();
            if state.refused_keys.get(device).is_some_and(|refused| now - *refused < TimeDelta::seconds(60)) {
                return refuse("control-key-not-confirmed");
            }
            state.confirming = true;
            before
        };
        // `finally { confirming = false; }`, also when the route's future is dropped while the user is asked.
        let confirming = Confirming(self.clone());
        let approved = (self.confirm)(device.to_string(), fingerprint.clone()).await;
        let saved = {
            let mut state = self.lock();
            match self.require(&state, device, "control") {
                Err(refusal) => Err(refusal),
                Ok(current) if current != before => refuse("control-key-not-confirmed"),
                Ok(_) if !approved => {
                    state.refused_keys.insert(device.to_string(), (self.now)());
                    refuse("control-key-not-confirmed")
                }
                Ok(current) => {
                    let mut copy = state.grants.clone();
                    copy.insert(device.to_string(), ScreenDeviceGrant { control_key_public: Some(to_base64(&key)), ..current });
                    self.save(&mut state, copy).map_err(Refusal::Io)
                }
            }
        };
        if saved.is_ok() {
            self.changed();
        }
        drop(confirming);
        saved.map(|()| MobileReply::new(200, wire::fingerprint(&fingerprint)))
    }

    async fn signal(self: &Arc<Self>, device: &str, raw: &str) -> Result<()> {
        if raw.len() > 65536 {
            return Ok(());
        }
        let Ok(signal) = serde_json::from_str::<Value>(raw) else { return Ok(()) };
        let Some(id) = json::text(&signal, "sessionId").map(str::to_string) else { return Ok(()) };
        if !self.lock().sessions.get(&id).is_some_and(|s| s.value.device_id == device) {
            return Ok(());
        }
        match json::text(&signal, "type") {
            Some("screen-session-end") if matches!(json::text(&signal, "reason"), Some("user-stop" | "background" | "peer-failed")) => {
                self.stop(&id, "peer-left", false).await;
            }
            Some("screen-background") if signal.get("background").is_some_and(Value::is_boolean) => {
                let background = signal.get("background") == Some(&Value::Bool(true));
                let now = (self.now)();
                if let Some(session) = self.lock().sessions.get_mut(&id) {
                    session.background = background.then_some(now);
                }
            }
            Some("screen-answer") if json::text(&signal, "sdp").is_some_and(|sdp| !sdp.is_empty()) => {
                self.platform.receive_signal(id, signal).await?;
            }
            Some("screen-ice") if json::text(&signal, "candidate").is_some_and(|c| mighty_core::wire::utf16_len(c) <= 4096) => {
                self.platform.receive_signal(id, signal).await?;
            }
            _ => {}
        }
        Ok(())
    }

    /// `MayControl`: the session is live, is a control session of a paired device whose grant still allows control, no key waits for
    /// the user, the PC is unlocked with no password field focused, and its own input was more than two seconds ago.
    fn may_control(&self, session_id: &str, mode: &str, device: &str) -> bool {
        let safety = self.platform.safety();
        let state = self.lock();
        state.sessions.contains_key(session_id)
            && mode == "control"
            && policy::grant_allows(state.grants.get(device), "control")
            && (self.trusted)(device)
            && !state.confirming
            && !safety.locked
            && !safety.secure_input
            && (self.now)() - safety.last_local_input >= TimeDelta::seconds(policy::LOCAL_INPUT_PAUSE_SECONDS)
    }

    async fn input(self: &Arc<Self>, session_id: &str, input: &str) {
        let Some(value) = wire::parse(input) else { return };
        let (live, cancel, clipboard) = {
            let now = (self.now)();
            let mut state = self.lock();
            let Some(session) = state.sessions.get_mut(session_id) else { return };
            session.activity = now;
            (session.value.clone(), session.cancel.clone(), session.clipboard.clone())
        };
        tokio::select! {
            _ = cancel.cancelled() => {}
            _ = self.deliver(&live, value, &clipboard) => {}
        }
    }

    /// `InputAsync`'s body: a platform failure ends the frame (C#'s caught `IOException` and `InvalidOperationException`).
    async fn deliver(self: &Arc<Self>, live: &ScreenLiveSession, value: Value, clipboard: &Mutex<ScreenClipboard>) -> Result<()> {
        let id = live.session_id.clone();
        let (mode, device) = (live.mode.as_str(), live.device_id.as_str());
        let kind = json::text(&value, "t").unwrap_or("").to_string();
        let marker = json::text(&value, "marker").map(str::to_string);
        if kind == "drag" && json::text(&value, "phase") == Some("end") {
            return self.platform.release_buttons(id).await;
        }
        if kind == "zoom" || kind == "display" {
            {
                let safety = self.platform.safety();
                let state = self.lock();
                if !(self.trusted)(device)
                    || !policy::grant_allows(state.grants.get(device), mode)
                    || state.confirming
                    || safety.locked
                    || safety.secure_input
                    || mode != "control" && state.sessions.len() != 1
                {
                    return Ok(());
                }
            }
            if kind == "zoom" {
                if let Some(region) = value.get("region").and_then(ScreenRegion::parse) {
                    self.platform.set_region(region);
                }
                return Ok(());
            }
            // A switch that cannot start anywhere ends every session, as the Mac's does (M/MightyCore/Remote/
            // ScreenShareEngine.swift:642-648).
            let wanted = int32(&value, "displayId").unwrap_or_default();
            let Ok(Some(display)) = self.capture(wanted).await else {
                // On a task of its own: ending every session cancels this one, whose input stops being waited for here.
                let inner = self.clone();
                let _ = tokio::spawn(async move { inner.end_all("display-gone", false).await }).await;
                return Ok(());
            };
            for session in self.lock().sessions.values_mut() {
                session.value.display_id = display;
            }
            self.changed();
            return Ok(());
        }
        if !self.may_control(&id, mode, device) {
            if let Some(marker) = marker.filter(|_| kind == "tap") {
                self.platform.send_data(id, wire::marker(&marker, false)).await?;
            }
            return Ok(());
        }
        if kind == "clipboard" {
            let text = clipboard.lock().unwrap_or_else(PoisonError::into_inner).receive(&value, (self.now)());
            if let Some(text) = text
                && self.may_control(&id, mode, device)
            {
                self.platform.write_clipboard(text).await?;
            }
            return Ok(());
        }
        if kind == "clipboard-request" {
            let text = self.platform.read_clipboard().await?;
            if !self.may_control(&id, mode, device) {
                return Ok(());
            }
            for chunk in ScreenClipboard::encode(text.as_deref()) {
                if !self.may_control(&id, mode, device) {
                    return Ok(());
                }
                self.platform.send_data(id.clone(), chunk).await?;
            }
            return Ok(());
        }
        self.platform.inject(id.clone(), value.clone()).await?;
        if let Some(marker) = marker.filter(|_| kind == "tap") {
            let shown = self.may_control(&id, mode, device) && self.platform.show_marker(id.clone(), value).await?;
            self.platform.send_data(id, wire::marker(&marker, shown)).await?;
        }
        Ok(())
    }

    async fn end_all(self: &Arc<Self>, reason: &str, killed: bool) {
        let stopped: Vec<Session> = {
            let mut state = self.lock();
            state.restarts.clear();
            state.sessions.drain(..).map(|(_, session)| session).collect()
        };
        for session in &stopped {
            session.cancel.cancel();
        }
        // Each peer is closed even when the native gate failed.
        let _ = self.platform.halt();
        let mut each = JoinSet::new();
        for session in stopped {
            let (inner, reason) = (self.clone(), reason.to_string());
            each.spawn(async move { inner.close_session(session.value, &reason, killed).await });
        }
        each.join_all().await;
        self.changed();
    }

    /// `Capture`: capture on the display a phone means (`policy::display_for`), picked once more when that one went away while capture
    /// moved to it, as undocking removes monitors one after another. None when it could start nowhere.
    async fn capture(&self, wanted: i32) -> Result<Option<i32>> {
        for _ in 0..2 {
            let Some(display) = policy::display_for(&self.platform.displays(), wanted) else { return Ok(None) };
            if self.platform.set_display(display.display_id).await? {
                return Ok(Some(display.display_id));
            }
        }
        Ok(None)
    }

    async fn display_lost(self: &Arc<Self>) {
        let Some(lost) = self.lock().sessions.values().next().map(|s| s.value.display_id) else { return };
        let safety = self.platform.safety();
        if safety.locked || safety.secure_input {
            self.end_all(if safety.locked { "lock-screen" } else { "secure-input" }, true).await;
            return;
        }
        let again = {
            let now = (self.now)();
            policy::may_restart_capture(&mut self.lock().restarts, now)
        };
        let moved = if again {
            tokio::select! {
                _ = self.lifetime.cancelled() => None,
                display = self.capture(lost) => display.ok().flatten(),
            }
        } else {
            None
        };
        let Some(display) = moved else {
            self.end_all("display-gone", false).await;
            return;
        };
        let phones: Vec<(String, Option<ScreenDeviceGrant>)> = {
            let mut state = self.lock();
            for session in state.sessions.values_mut() {
                session.value.display_id = display;
            }
            let mut phones = Vec::new();
            for session in state.sessions.values() {
                let phone = (session.value.device_id.clone(), state.grants.get(&session.value.device_id).cloned());
                if !phones.contains(&phone) {
                    phones.push(phone);
                }
            }
            phones
        };
        self.changed();
        for (device, grant) in phones {
            let message = {
                let ice = self.lock().ice.clone();
                let allowed = grant.as_ref().is_some_and(|g| g.allowed);
                wire::grant(allowed, grant.as_ref().map_or("none", |g| g.grant.as_str()), &self.platform.displays(), &ice)
            };
            let _ = timeout(Duration::from_secs(1), (self.send)(device, message)).await;
        }
    }

    async fn stop(self: &Arc<Self>, id: &str, reason: &str, killed: bool) {
        let Some(session) = self.lock().sessions.shift_remove(id) else { return };
        session.cancel.cancel();
        self.close_session(session.value, reason, killed).await;
        self.changed();
    }

    /// `CloseSession`: the peer stopped and its buttons released within a second (or the pixel gate halted), and only then the phone
    /// told, best effort: a revoked phone may stop reading its relay socket on purpose.
    async fn close_session(&self, session: ScreenLiveSession, reason: &str, killed: bool) {
        let id = session.session_id;
        let closed = timeout(Duration::from_secs(1), async {
            let (stopped, released) = tokio::join!(self.platform.stop_peer(id.clone()), self.platform.release_buttons(id.clone()));
            stopped.is_ok() && released.is_ok()
        })
        .await;
        if closed != Ok(true) {
            let _ = self.platform.halt();
        }
        let _ = timeout(Duration::from_secs(1), (self.send)(session.device_id, wire::session_end(killed, &id, reason))).await;
    }

    async fn set_ice_servers(self: &Arc<Self>, servers: Vec<ScreenIceServer>) {
        self.lock().ice = servers.clone();
        let mut restart = JsonOut::new();
        restart.begin_object();
        restart.prop_str("type", Some("host-ice-restart"));
        restart.name("iceServers");
        wire::write_ice(&mut restart, &servers);
        restart.end_object();
        let restart: Value = serde_json::from_str(restart.as_str()).expect("the hub's own JSON reads");
        let mut each = JoinSet::new();
        for session in self.sessions() {
            let (inner, servers, restart) = (self.clone(), servers.clone(), restart.clone());
            each.spawn(async move {
                let mut out = JsonOut::new();
                out.begin_object();
                out.prop_str("type", Some("screen-grant"));
                out.prop_str("sessionId", Some(&session.session_id));
                out.prop_bool("allowed", true);
                out.prop_str("grant", Some(&session.mode));
                out.name("iceServers");
                wire::write_ice(&mut out, &servers);
                out.end_object();
                let sent = timeout(Duration::from_secs(2), (inner.send)(session.device_id.clone(), out.finish())).await;
                let restarted = match sent {
                    Ok(Ok(())) => {
                        let signal = inner.platform.receive_signal(session.session_id.clone(), restart);
                        tokio::select! {
                            _ = inner.lifetime.cancelled() => false,
                            done = timeout(Duration::from_secs(8), signal) => matches!(done, Ok(Ok(()))),
                        }
                    }
                    _ => false,
                };
                if !restarted {
                    inner.stop(&session.session_id, "peer-left", false).await;
                }
            });
        }
        each.join_all().await;
    }

    /// `Watch`: every 250 ms, a locked PC or a focused password field kills every session at once; otherwise a session ends 30 s
    /// into the background (`background`), 30 s without its peer connecting (`peer-left`), or idle 600 s (control) or 1800 s (view)
    /// (`idle-timeout`).
    async fn watch(self: Arc<Self>) {
        loop {
            tokio::select! {
                _ = self.lifetime.cancelled() => return,
                _ = tokio::time::sleep(Duration::from_millis(250)) => {}
            }
            let safety = self.platform.safety();
            if safety.locked || safety.secure_input {
                if !self.lock().sessions.is_empty() {
                    self.end_all(if safety.locked { "lock-screen" } else { "secure-input" }, true).await;
                }
                continue;
            }
            let expired: Vec<(String, &'static str)> = {
                let now = (self.now)();
                let since = |at: DateTime<Utc>| now - at;
                self.lock()
                    .sessions
                    .values()
                    .filter_map(|s| {
                        let idle = if s.value.mode == "control" { policy::CONTROL_IDLE_SECONDS } else { policy::VIEW_IDLE_SECONDS };
                        let reason = if s.background.is_some_and(|b| since(b) > TimeDelta::seconds(policy::BACKGROUND_SECONDS)) {
                            "background"
                        } else if !s.connected && since(s.value.started_at) > TimeDelta::seconds(policy::BACKGROUND_SECONDS) {
                            "peer-left"
                        } else if since(s.activity) > TimeDelta::seconds(idle) {
                            "idle-timeout"
                        } else {
                            return None;
                        };
                        Some((s.value.session_id.clone(), reason))
                    })
                    .collect()
            };
            for (id, reason) in expired {
                self.stop(&id, reason, false).await;
            }
        }
    }
}

/// Clears `confirming` when the enrollment ends, however it ends.
struct Confirming(Arc<Inner>);

impl Drop for Confirming {
    fn drop(&mut self) {
        self.0.lock().confirming = false;
    }
}

/// `MobileIdentity.SecureDirectory`'s portable half: the folder made, and refused when it is a link (or any other reparse point);
/// owner-only on Unix. Windows' owner-only ACL is the identity folder's (W8-A's pairing store makes the folder this file shares).
fn secure_directory(path: &Path) -> io::Result<()> {
    std::fs::create_dir_all(path)?;
    if linked(&std::fs::symlink_metadata(path)?) {
        return Err(io::Error::other("Refusing linked mobile identity directory."));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

/// `FileAttributes.ReparsePoint`: a link, a junction, or any other reparse point.
fn linked(meta: &std::fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt as _;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
        if meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return true;
        }
    }
    meta.file_type().is_symlink()
}

/// The constructor's read of `screen-devices.json`: a file over 64 KiB or a link is an error; a file that does not read (not JSON, a
/// field of the wrong type) gives no grants; rows with a bad id, an unknown grant, or a control key that is not a P-256 point on a
/// control grant are dropped; then the first 32, each device once.
fn load(path: &Path) -> io::Result<IndexMap<String, ScreenDeviceGrant>> {
    let meta = match std::fs::symlink_metadata(path) {
        Ok(meta) if !meta.is_dir() => meta,
        _ => return Ok(IndexMap::new()),
    };
    if meta.len() > 65536 || linked(&meta) {
        return Err(io::Error::other("Invalid screen sharing settings."));
    }
    let Ok(bytes) = std::fs::read(path) else { return Ok(IndexMap::new()) };
    let text = String::from_utf8_lossy(&bytes);
    let text = text.strip_prefix('\u{feff}').unwrap_or(&text);
    let Ok(value) = serde_json::from_str::<Value>(text) else { return Ok(IndexMap::new()) };
    if json::depth(&value) > json::MAXIMUM_DEPTH {
        return Ok(IndexMap::new());
    }
    let Ok(Some(rows)) = json::list(Some(&value), StoredGrant::read) else { return Ok(IndexMap::new()) };
    let mut grants = IndexMap::new();
    let valid = rows.into_iter().flatten().filter_map(|row| {
        let (device_id, grant) = (row.device_id?, row.grant?);
        let key_ok = match row.control_key_public.as_deref() {
            None => true,
            Some(key) => grant == "control" && valid_stored_key(key),
        };
        (client_id(&device_id) && matches!(grant.as_str(), "none" | "view" | "control") && key_ok).then_some(ScreenDeviceGrant {
            device_id,
            allowed: row.allowed,
            grant,
            control_key_public: row.control_key_public,
        })
    });
    for grant in valid.take(32) {
        if !grants.contains_key(&grant.device_id) {
            grants.insert(grant.device_id.clone(), grant);
        }
    }
    Ok(grants)
}

/// `ValidStoredKey`: base64 of at most 128 characters holding a P-256 point.
fn valid_stored_key(text: &str) -> bool {
    mighty_core::wire::utf16_len(text) <= 128 && from_base64(text).is_some_and(|key| policy::valid_key(&key))
}
