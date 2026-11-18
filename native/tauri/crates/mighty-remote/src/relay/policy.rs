//! When the relay link redials (C# R\RelayLink.cs `RelayNetworkPath`, `RelayLinkPolicy`; plan §1.1), pure: a path change settles 1.5 s
//! (5 s at most), a path-triggered redial waits a gap of 5 s doubling to 30 s, a wake redials without pacing, no network goes offline;
//! the control socket's keepalive is a ping every 20 s with a 10 s pong timeout; a reconnect waits 1 s doubling to 30 s, reset after a
//! connect. The path events come from mighty-platform's `Network` port (`on_change`, `on_resume`). Owner: W8-A.
//!
//! Times are milliseconds on any monotonic clock the caller keeps (the host's `tokio::time::Instant` since its start; the checks their
//! own numbers), so the decisions are plain functions of their inputs.

use std::net::IpAddr;
use std::time::Duration;

use mighty_platform::Adapters;

pub const PATH_SETTLE: Duration = Duration::from_millis(1500);
pub const PATH_SETTLE_MAXIMUM: Duration = Duration::from_secs(5);
pub const REDIAL_GAP: Duration = Duration::from_secs(5);
pub const REDIAL_GAP_MAXIMUM: Duration = Duration::from_secs(30);
pub const KEEPALIVE_INTERVAL: Duration = Duration::from_secs(20);
pub const KEEPALIVE_TIMEOUT: Duration = Duration::from_secs(10);
pub const RECONNECT_FIRST: Duration = Duration::from_secs(1);
pub const RECONNECT_MAXIMUM: Duration = Duration::from_secs(30);

/// `RelayLinkPolicy.PathAction`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkAction {
    None,
    Offline,
    Reconnect,
}

/// What kind of adapter carries a path (C# `NetworkInterfaceType` as `RelayNetworkPath.From` reads it).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkKind {
    Loopback,
    Tunnel,
    Other,
}

/// C# `RelayNetworkPath(Satisfied, Interfaces, Gateways)`: whether there is a network at all, and which interfaces and routers carry it
/// (sorted, distinct). A Wi-Fi switch, a VPN coming up or a cable pulled changes one of these.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct NetworkPath {
    pub satisfied: bool,
    pub interfaces: Vec<String>,
    pub gateways: Vec<String>,
}

impl NetworkPath {
    pub fn new(satisfied: bool, interfaces: &[&str], gateways: &[&str]) -> NetworkPath {
        NetworkPath {
            satisfied,
            interfaces: interfaces.iter().map(|s| s.to_string()).collect(),
            gateways: gateways.iter().map(|s| s.to_string()).collect(),
        }
    }

    /// `RelayNetworkPath.From`: the adapters that can carry the socket out — those with a default gateway, loopback and tunnels aside.
    /// A Hyper-V, WSL or Docker switch has no gateway on the host's side, so starting one does not cost the phones their tunnels; a VPN
    /// that takes the default route does change the path.
    pub fn from<'a>(up: impl IntoIterator<Item = (&'a str, LinkKind, &'a [IpAddr])>, available: bool) -> NetworkPath {
        let gateway = |address: &IpAddr| !address.is_unspecified();
        let carrying: Vec<(&str, &[IpAddr])> = up
            .into_iter()
            .filter(|(_, kind, gateways)| *kind == LinkKind::Other && gateways.iter().any(gateway))
            .map(|(id, _, gateways)| (id, gateways))
            .collect();
        let mut gateways: Vec<String> = carrying.iter().flat_map(|(_, g)| g.iter().filter(|a| gateway(a)).map(IpAddr::to_string)).collect();
        gateways.sort();
        gateways.dedup();
        let mut interfaces: Vec<String> = carrying.iter().map(|(id, _)| id.to_string()).collect();
        interfaces.sort();
        interfaces.dedup();
        NetworkPath { satisfied: !carrying.is_empty() && available, interfaces, gateways }
    }

    /// The path as mighty-platform's `Network` port reads it, which names each adapter that is up and its addresses but no gateways:
    /// an adapter carries the path when it has an address that is neither loopback, unspecified nor link-local (the nearest the port
    /// comes to "has a default gateway"); its gateways are its addresses. Unreadable adapters are left out, as C#'s `Current` leaves a
    /// read that throws.
    pub fn from_adapters(adapters: &Adapters) -> NetworkPath {
        let routable = |address: &IpAddr| match address {
            IpAddr::V4(v4) => !v4.is_loopback() && !v4.is_unspecified() && !v4.is_link_local(),
            IpAddr::V6(v6) => !v6.is_loopback() && !v6.is_unspecified() && (v6.segments()[0] & 0xffc0) != 0xfe80,
        };
        let carrying: Vec<(&str, Vec<IpAddr>)> = adapters
            .adapters
            .iter()
            .map(|a| (a.name.as_str(), a.addresses.iter().copied().filter(routable).collect::<Vec<_>>()))
            .filter(|(_, addresses)| !addresses.is_empty())
            .collect();
        let mut interfaces: Vec<String> = carrying.iter().map(|(name, _)| name.to_string()).collect();
        interfaces.sort();
        interfaces.dedup();
        let mut gateways: Vec<String> = carrying.iter().flat_map(|(_, a)| a.iter().map(IpAddr::to_string)).collect();
        gateways.sort();
        gateways.dedup();
        NetworkPath { satisfied: !carrying.is_empty(), interfaces, gateways }
    }
}

/// `RelayLinkPolicy.Action(old, next, forced, disturbed)`: `old` is the path the current connection was made on (None before the first
/// report), `next` the settled one (None when only a wake fired). No network reads as offline rather than as a reason to redial; any
/// other difference, or a disturbed window, means the old socket may be half-open, so it is replaced.
pub fn action(old: Option<&NetworkPath>, next: Option<&NetworkPath>, forced: bool, disturbed: bool) -> LinkAction {
    let Some(next) = next else { return if forced { LinkAction::Reconnect } else { LinkAction::None } };
    if !next.satisfied {
        return if old.is_some_and(|o| !o.satisfied) && !forced { LinkAction::None } else { LinkAction::Offline };
    }
    if forced || disturbed {
        return LinkAction::Reconnect;
    }
    match old {
        Some(old) if old == next => LinkAction::None,
        None => LinkAction::None,
        Some(_) => LinkAction::Reconnect,
    }
}

/// `RelayLinkPolicy.Window`: everything reported since the last decision, not only the last report — a Wi-Fi switch between two routers
/// on the same 192.168.x.1 reports wifi → nothing → wifi, and the settled path then equals the old one although the socket under it
/// is gone.
#[derive(Debug, Clone, Default)]
pub struct Window {
    /// The latest report, or None when only a wake opened the window.
    pub latest: Option<NetworkPath>,
    /// Some report differed from the settled path or had no network.
    pub disturbed: bool,
    /// A wake asked for a redial whatever the path does.
    pub forced: bool,
    /// When the first report or wake opened it (milliseconds).
    pub opened_at: Option<u64>,
}

impl Window {
    pub fn is_empty(&self) -> bool {
        self.latest.is_none() && !self.forced
    }

    pub fn report(&mut self, path: NetworkPath, settled: Option<&NetworkPath>, now: u64) {
        self.opened_at.get_or_insert(now);
        if settled.is_some_and(|s| path != *s || !path.satisfied) {
            self.disturbed = true;
        }
        self.latest = Some(path);
    }

    pub fn force(&mut self, now: u64) {
        self.opened_at.get_or_insert(now);
        self.forced = true;
    }

    /// How long to wait before deciding: the settle delay, cut short so the window never stays open longer than
    /// [`PATH_SETTLE_MAXIMUM`].
    pub fn delay(&self, now: u64) -> Duration {
        let Some(opened) = self.opened_at else { return PATH_SETTLE };
        let left = (opened + PATH_SETTLE_MAXIMUM.as_millis() as u64).saturating_sub(now);
        Duration::from_millis(left).min(PATH_SETTLE)
    }

    /// `RelayLinkPolicy.Action(old, window)`.
    pub fn action(&self, old: Option<&NetworkPath>) -> LinkAction {
        action(old, self.latest.as_ref(), self.forced, self.disturbed)
    }
}

/// `RelayLinkPolicy.Pacing`: the minimum gap between path-triggered redials. A redial more than the current gap plus the maximum after
/// the last one starts over at the minimum; one sooner doubles the gap.
#[derive(Debug, Clone, Default)]
pub struct Pacing {
    last: Option<u64>,
    streak: u32,
}

impl Pacing {
    pub fn gap(&self) -> Duration {
        let doubled = REDIAL_GAP.as_secs_f64() * 2f64.powi(self.streak.min(16) as i32);
        Duration::from_secs_f64(doubled.min(REDIAL_GAP_MAXIMUM.as_secs_f64()))
    }

    /// How long a redial wanted now has to wait.
    pub fn wait(&self, now: u64) -> Duration {
        match self.last {
            Some(at) => Duration::from_millis((at + self.gap().as_millis() as u64).saturating_sub(now)),
            None => Duration::ZERO,
        }
    }

    pub fn record(&mut self, now: u64) {
        let window = (self.gap() + REDIAL_GAP_MAXIMUM).as_millis() as u64;
        self.streak = match self.last {
            Some(at) if now.saturating_sub(at) < window => self.streak + 1,
            _ => 0,
        };
        self.last = Some(now);
    }
}

/// The control socket's reconnect wait after `seconds` (1 s doubling to 30 s).
pub fn next_reconnect(seconds: u32) -> u32 {
    (seconds * 2).min(RECONNECT_MAXIMUM.as_secs() as u32)
}
