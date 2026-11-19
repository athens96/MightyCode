//! The Tailscale direct path in the window (C# WinUI/MainWindow.MobileDirect.cs: the firewall rule's state, ports, add, remove and
//! re-check; MainWindow.MobileRemote.cs's direct rows' writes and `RemoveRuleAfterOff`; the direct half of MobileRelayHost: `ApplyDirect`,
//! re-detection every 15 s and on every address change) over mighty-remote's `direct::listener::DirectPath` and the platform's
//! `Network` and `Firewall`. Owner: W8-C.
//!
//! - The path exists only when the app may listen ([`Host::may_listen`]: `--allow-state-write`, never in a smoke) and listens only once
//!   the relay host is attached as its sink ([`attach`], W8-A's `mobile.rs`) and the remote and direct switches are on: Tailscale's
//!   addresses only, never a wildcard, each exclusively. Its endpoints feed `/m1/info` and the pairing URL ([`endpoints`]).
//! - The firewall rule (D4): read unelevated whenever Settings opens, and only with `--allow-state-write` (C-L5: without it the row
//!   reads unknown, where C# reads it in any run); changed only on the user's click and only when the app may change
//!   the machine — never in a smoke, whose step puts a recorder in ([`smoke_use_firewall`], C#'s `firewallRunner`) and never otherwise
//!   reads or changes this PC's firewall. Its ports are the direct port (remote and direct on) and remote sharing's
//!   (`remote::sharing_port`, W8-D). Only Core's words reach the row (`direct::strings::firewall_outcome`).

use std::sync::{Arc, Mutex, MutexGuard};

use mighty_core::model::MobileRemoteSettings;
use mighty_platform::{Firewall, FirewallRule, Platform};
use mighty_remote::direct::listener::{DirectPath, DirectPathOptions, DirectSettings, DirectSink, MagicDnsLookup};
use mighty_remote::direct::status::MobileDirectStatus;
use mighty_remote::direct::strings;

use crate::host::Host;

/// The window's direct state (C#'s `MainWindow` fields of MainWindow.MobileDirect.cs).
struct Direct {
    /// The path, when the app may listen.
    path: Option<DirectPath>,
    /// The platform's rule (`remote_ports`: the fake in a smoke, which is never used there).
    firewall: Option<Arc<dyn Firewall>>,
    /// C# `firewallRunner` as the smoke sets it: a recorder that starts nothing.
    smoke_firewall: Option<Arc<dyn Firewall>>,
    /// C# `firewallState` and `firewallRulePorts`: unknown (`Unreadable`) until read, and for a whole smoke that set no runner.
    rule: FirewallRule,
    /// C# `firewallMessage`: what the last add or remove that did not happen said, in Core's words only.
    message: Option<String>,
    /// C# `firewallBusy`: a change (or a re-check) is under way; Windows' prompt waits as long as the user does.
    busy: bool,
    /// C# `mobileDirectStatusOverride`: the smoke's fixed status, never this PC's.
    status_override: Option<MobileDirectStatus>,
}

static DIRECT: Mutex<Direct> = Mutex::new(Direct {
    path: None,
    firewall: None,
    smoke_firewall: None,
    rule: FirewallRule::Unreadable,
    message: None,
    busy: false,
    status_override: None,
});

/// The direct switch's and the port's writes, one after another (C#'s `writing`: a press made while one applies is applied next).
static WRITES: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn direct() -> MutexGuard<'static, Direct> {
    DIRECT.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Settings redrawn (C#'s `FirewallChanged` and `StatusChanged`), never under this module's lock.
fn redraw(host: &Host) {
    crate::native::touch(host, crate::settings::SURFACE);
}

fn saved(host: &Host) -> MobileRemoteSettings {
    host.view().snapshot.mobile_remote.clone().unwrap_or_default()
}

fn settings_of(saved: &MobileRemoteSettings) -> DirectSettings {
    let port = u16::try_from(saved.direct_port).ok().filter(|p| *p >= 1024).unwrap_or(mighty_remote::direct::wire::DEFAULT_PORT);
    DirectSettings { enabled: saved.enabled, direct_enabled: saved.direct_enabled, port }
}

/// The direct path's start with the app: the firewall port kept, and the path made when the app may listen (it listens once
/// [`attach`]ed). A smoke makes no path.
pub fn start(host: &Arc<Host>, platform: &Platform) {
    let ports = crate::platform::remote_ports(platform, host.smoke.is_some());
    let path = host.may_listen().then(|| {
        let log = {
            let host = Arc::downgrade(host);
            Arc::new(move |line: &str| {
                if let Some(host) = host.upgrade() {
                    host.log.line(line);
                }
            })
        };
        let options = DirectPathOptions {
            needs_utun: platform.profile.tailscale_needs_utun,
            magic_dns: platform.profile.direct_advertises_magic_dns.then(|| magic_dns(platform.processes.clone())),
            ..DirectPathOptions::default()
        };
        let path = DirectPath::new(ports.network.clone(), tauri::async_runtime::handle().inner().clone(), options, log);
        let weak = Arc::downgrade(host);
        path.on_published(move || {
            if let Some(host) = weak.upgrade() {
                redraw(&host);
            }
        });
        path
    });
    let mut state = direct();
    state.firewall = Some(ports.firewall);
    state.path = path;
}

/// Whether this platform's direct path has a firewall rule to show and change (`PlatformProfile::direct_connection_firewall`: Windows;
/// the Mac shows no firewall row, M-C).
pub fn has_firewall(host: &Host) -> bool {
    let _ = host;
    crate::platform::profile().direct_connection_firewall
}

/// The MagicDNS lookup where `PlatformProfile::direct_advertises_magic_dns` (the Mac; M-C): the Tailscale CLI from its places and this
/// process's PATH, run through the process host (mighty-remote's `MagicDnsLookup::tailscale`).
fn magic_dns(processes: Arc<dyn mighty_platform::ProcessHost>) -> MagicDnsLookup {
    let candidates = mighty_remote::direct::wire::tailscale_candidates(&std::env::var("PATH").unwrap_or_default());
    MagicDnsLookup::tailscale(processes, candidates)
}

/// The app quits: every direct listener closed and every direct phone ended.
pub fn shutdown(host: &Host) {
    let _ = host;
    let path = direct().path.take();
    if let Some(path) = path {
        path.dispose();
    }
}

/// W8-A's relay host is up: it runs the admitted sockets, and the path follows the saved settings from now on. Nothing happens when
/// the app may not listen.
#[allow(dead_code, reason = "W8-A's mobile.rs attaches its relay host")]
pub fn attach(host: &Arc<Host>, sink: Arc<dyn DirectSink>) {
    let Some(path) = direct().path.clone() else { return };
    path.set_sink(Some(sink));
    path.store_settings(settings_of(&saved(host)));
    tauri::async_runtime::spawn(async move {
        path.refresh(true).await;
    });
}

/// The relay host is gone: nothing more is admitted and the path stops listening (its phones end).
#[allow(dead_code, reason = "W8-A's mobile.rs detaches its relay host")]
pub fn detach(host: &Host) {
    let _ = host;
    let Some(path) = direct().path.clone() else { return };
    path.set_sink(None);
    tauri::async_runtime::spawn(async move {
        path.refresh(true).await;
    });
}

/// The saved `mobileRemote` settings applied to the path (C# `MobileRelayHost.ApplyAsync`'s direct half): kept by the path at once, so
/// writes land in the order they were made, then a pass on a task (its adapters read off the caller); W8-A's writes call it right
/// after saving, as the direct rows' do.
pub fn apply(host: &Host) {
    let Some(path) = direct().path.clone() else { return };
    let force = path.store_settings(settings_of(&saved(host)));
    tauri::async_runtime::spawn(async move {
        path.refresh(force).await;
    });
}

/// The endpoints the direct path advertises (`/m1/info`'s `direct`, auth_ok's `direct`, the pairing URL's `&direct=`): at most 8, IPv6
/// in brackets; none while nothing listens.
#[allow(dead_code, reason = "W8-A (auth_ok, the pairing URL) and W8-B (`/m1/info`) read them")]
pub fn endpoints(host: &Host) -> Vec<String> {
    let _ = host;
    direct().path.as_ref().map(DirectPath::endpoints).unwrap_or_default()
}

/// C# `MobileDirectStatusNow`: the smoke's fixed status, else the path's, else off.
pub fn status(host: &Host) -> MobileDirectStatus {
    let _ = host;
    let state = direct();
    state.status_override.clone().or_else(|| state.path.as_ref().map(DirectPath::status)).unwrap_or_default()
}

/// C# `FirewallPorts` (`RemoteWorkspaceFeatures.FirewallPorts`): the phone's direct port while the remote and its direct switch are
/// on, and the sharing port while this PC shares its workspaces.
pub fn firewall_ports(host: &Host) -> Vec<u16> {
    let saved = saved(host);
    let direct = (saved.enabled && saved.direct_enabled).then(|| u16::try_from(saved.direct_port).ok()).flatten();
    mighty_remote::remote::features::firewall_ports(direct, crate::remote::sharing_port(host))
}

/// The firewall row's state as Settings draws it (both boxes' rows: 모바일 원격 here and W8-D's 원격 연결).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FirewallView {
    pub rule: FirewallRule,
    pub message: Option<String>,
    pub busy: bool,
    /// Whether a press may run netsh: not in a smoke (its recorder aside, the button stays off there as C#'s does), and only with
    /// `--allow-state-write` (D4).
    pub may_change: bool,
}

pub fn firewall_view(host: &Host) -> FirewallView {
    let may_change = host.may_change_machine();
    let state = direct();
    FirewallView { rule: state.rule.clone(), message: state.message.clone(), busy: state.busy, may_change }
}

/// This executable's path, the program the rule must name.
fn program() -> String {
    std::env::current_exe().map(|exe| exe.to_string_lossy().into_owned()).unwrap_or_default()
}

/// C# `FirewallNeedsUpdateWords`: " · " and the update words when the rule there does not open every port in use now, or names another
/// program (or one that could not be read).
pub fn firewall_needs_update_words(host: &Host) -> String {
    let rule = direct().rule.clone();
    if rule.needs_update(&firewall_ports(host), &program()) {
        format!(" · {}", host.locale.get("remote.windows.firewall.needsUpdate"))
    } else {
        String::new()
    }
}

/// The runner a press or a read uses: the smoke's recorder, else the platform's when the app may change the machine; none otherwise (a
/// smoke with no recorder; a run without `--allow-state-write`, which per D4 does not even run the unelevated `netsh show`, so its row
/// reads as unknown — a deviation from C#, which reads the rule in any run).
fn runner(host: &Host) -> Option<Arc<dyn Firewall>> {
    let may_change = host.may_change_machine();
    if !has_firewall(host) {
        return None;
    }
    let state = direct();
    if host.smoke.is_some() {
        return state.smoke_firewall.clone();
    }
    state.firewall.clone().filter(|_| may_change)
}

/// C# `RefreshFirewallState`: whether the rule exists and the ports it opens, unelevated; anything unexpected reads as unknown. A smoke
/// that set no recorder never reads this PC's rule.
pub async fn refresh_firewall_state(host: &Host) {
    let Some(firewall) = runner(host) else { return };
    let rule = firewall.read().await.unwrap_or(FirewallRule::Unreadable);
    direct().rule = rule;
}

/// Settings opened (C#'s Loaded: `if (!firewallBusy) await RefreshFirewallState();`): the rule read again, off the caller.
pub fn settings_opened(host: &Arc<Host>) {
    // C#'s Loaded returns at once in a smoke: the box is drawn with its controls off and nothing is read. A platform with no firewall
    // step reads nothing either.
    if host.smoke.is_some() || direct().busy || !has_firewall(host) {
        return;
    }
    let host = host.clone();
    tauri::async_runtime::spawn(async move {
        refresh_firewall_state(&host).await;
        redraw(&host);
    });
}

/// A change (or a re-check) under way: the busy mark, cleared when it is dropped — however the change ends, its future dropped included.
struct Busy<'a>(&'a Host);

impl Drop for Busy<'_> {
    fn drop(&mut self) {
        direct().busy = false;
        redraw(self.0);
    }
}

/// Marks a change under way; none when one already is.
fn begin(host: &Host) -> Option<Busy<'_>> {
    {
        let mut state = direct();
        if state.busy {
            return None;
        }
        state.busy = true;
    }
    redraw(host);
    Some(Busy(host))
}

/// C# `ChangeFirewallRule`: the row's button (and a switch turned off by the user): one netsh command, elevated through Windows' own
/// prompt, then the rule read again. Only Core's outcome words are kept; a press while a change waits does nothing. Real netsh runs only
/// with `--allow-state-write` (D4); a smoke's recorder starts nothing.
pub async fn change_firewall_rule(host: &Host, add: bool) {
    let Some(firewall) = runner(host) else { return };
    if host.smoke.is_none() && !host.may_change_machine() {
        return;
    }
    let Some(_busy) = begin(host) else { return };
    let ports = firewall_ports(host);
    let result = if add { firewall.add(ports).await } else { firewall.remove().await };
    let result = result.unwrap_or_else(|_| mighty_platform::FirewallChange::Failed(String::new()));
    direct().message = strings::firewall_outcome(&host.locale, &result);
    refresh_firewall_state(host).await;
}

/// C# `RecheckFirewallRule`: the row's button while the rule could not be read — the unelevated query again, busy like a change.
pub async fn recheck_firewall_rule(host: &Host) {
    if runner(host).is_none() {
        return;
    }
    let Some(_busy) = begin(host) else { return };
    direct().message = None;
    refresh_firewall_state(host).await;
}

/// C# `RemoveRuleAfterOff` / `RemoveFirewallRuleIfUnneeded`: turning the remote or its direct path off takes the rule away when nothing
/// needs it any more, from that same click (W8-A's remote switch calls it after its off-write, as the direct switch's does).
pub async fn switched_off(host: &Host) {
    let present = matches!(direct().rule, FirewallRule::Present { .. });
    if present && firewall_ports(host).is_empty() {
        change_firewall_rule(host, false).await;
    }
    redraw(host);
}

/// C# `Write` for the direct rows: the change saved (the host's store writes the profile only with `--allow-state-write`), the path
/// told, Settings redrawn; writes run one after another, so the last press is what stays.
pub async fn write(host: &Host, change: impl FnOnce(&mut MobileRemoteSettings)) -> Result<(), String> {
    let _writing = WRITES.lock().await;
    let mut next = saved(host);
    change(&mut next);
    host.view().update(|state| state.mobile_remote = Some(next))?;
    apply(host);
    redraw(host);
    Ok(())
}

/// Whether a direct-row write is under way (C#'s `writing`: the port's apply button waits for it).
pub fn writing() -> bool {
    WRITES.try_lock().is_err()
}

/// The direct switch pressed: on is a write; off is a write, then the rule looked at (C# `SwitchedOff`).
pub fn switch_pressed(host: &Arc<Host>, on: bool) {
    let host = host.clone();
    crate::native::act(&host.clone(), async move {
        write(&host, |saved| saved.direct_enabled = on).await?;
        if !on {
            switched_off(&host).await;
        }
        Ok(())
    });
}

/// The port's apply button (and Enter in its field): the typed port when it is one the setting takes and another than the saved one.
pub fn apply_port(host: &Arc<Host>, typed: &str) {
    let Some(port) = typed_port(typed).filter(|port| *port != saved(host).direct_port) else { return };
    let host = host.clone();
    crate::native::act(&host.clone(), async move { write(&host, |saved| saved.direct_port = port).await });
}

/// C# `TypedPort`: the port as typed (ASCII digits only) when the setting takes it (1024-65535).
pub fn typed_port(typed: &str) -> Option<i32> {
    let typed = typed.trim();
    if typed.is_empty() || !typed.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    typed.parse::<i32>().ok().filter(|port| mighty_remote::direct::wire::valid_port(i64::from(*port)))
}

/// The firewall row's add button (C# MainWindow.RemoteWorkspaces.cs:277; one row for sharing and the phone's direct path since the
/// Settings tidy-up): the unelevated query again while the rule could not be read, else the rule added or updated in place for the
/// ports in use now.
pub fn firewall_add_pressed(host: &Arc<Host>) {
    let host = host.clone();
    crate::native::act(&host.clone(), async move {
        let unknown = direct().rule == FirewallRule::Unreadable;
        if unknown {
            recheck_firewall_rule(&host).await;
        } else {
            change_firewall_rule(&host, true).await;
        }
        Ok(())
    });
}

/// The firewall row's delete button: the rule taken away (it closes the phone's direct port and the sharing port both).
pub fn firewall_remove_pressed(host: &Arc<Host>) {
    let host = host.clone();
    crate::native::act(&host.clone(), async move {
        change_firewall_rule(&host, false).await;
        Ok(())
    });
}

/// The smoke's seams (C# `RunMobileDirectSmoke` sets `firewallRunner` and `mobileDirectStatusOverride`, and puts both back in its
/// `finally`): a recorder for the rule, a fixed status; `None` puts each back and forgets the rule's state.
pub fn smoke_use_firewall(firewall: Option<Arc<dyn Firewall>>, status: Option<MobileDirectStatus>) {
    let mut state = direct();
    let restoring = firewall.is_none();
    state.smoke_firewall = firewall;
    state.status_override = status;
    if restoring {
        state.rule = FirewallRule::Unreadable;
        state.message = None;
    }
}
