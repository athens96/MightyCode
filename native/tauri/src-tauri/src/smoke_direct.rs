//! The remote step's direct part (C# WinUI/MainWindow.MobileDirectSmoke.cs `RunMobileDirectSmoke`; live key `mobileDirect`), first of
//! `Stage::Remote` (after the updates step). The direct rows of 모바일 원격, read as the page would get them, over a fixed fake status
//! and a firewall runner that records what it would run and starts nothing: the switch, the port and its apply button, the status line,
//! the rule read (unelevated) before any click, then the add and remove commands exactly as the builder makes them, and a declined and a
//! timed-out prompt ending in Core's words with the button free again, an unreadable rule offering to read it again. Nothing on this PC
//! is probed and no netsh runs. Then (Rust's own key `listener`) the direct path on loopback port 0 through the fake network — never a
//! real interface — upgrading a phone's request for a stand-in sink. Its report is the stage report's `mobileDirect`. Owner: W8-C.

use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use mighty_native::surface::{SurfaceModel, Surfaces};
use mighty_native::ui::Ui;
use mighty_native::view::{Place, Section, View};
use mighty_platform::fake::{FakeFirewall, FakeNetwork, RecordingNetsh};
use mighty_platform::firewall::netsh;
use mighty_platform::firewall::{NetshFailure, NetshRunner};
use mighty_platform::{BoxFuture, FirewallChange, FirewallRule};
use mighty_remote::direct::listener::{DirectPath, DirectPathOptions, DirectSettings, DirectSink, DirectSocket};
use mighty_remote::direct::status::{DirectState, MobileDirectStatus};
use mighty_remote::direct::{strings, wire};
use serde_json::{Map, Value, json};

use crate::host::Host;

/// The keys the part reports, in C#'s order, then the Rust-only one.
const KEYS: [&str; 7] = ["rows", "status", "firewallRow", "addCommand", "removeCommand", "outcomes", "listener"];

/// Starts the direct part; false when it does not run (then the smoke goes on to the remote workspaces part). It reports once through
/// `Smoke::probed`.
pub fn start(host: &Arc<Host>) -> bool {
    if host.smoke.is_none() {
        return false;
    }
    host.log.line("smoke: remote step, direct part (a recording firewall runner, the fake network on loopback)");
    let host = host.clone();
    tauri::async_runtime::spawn(async move {
        let mut checks = Map::new();
        let outcome = run(&host, &mut checks).await;
        let mut report = Value::Object(checks);
        report["passed"] = json!(outcome.is_ok() && KEYS.iter().all(|key| report.get(*key) == Some(&Value::Bool(true))));
        if let Err(error) = outcome {
            host.log.line(&format!("smoke: direct part failed: {error}"));
            report["error"] = json!(error);
        }
        if let Some(smoke) = &host.smoke {
            smoke.probed(&host, report);
        }
    });
    true
}

/// The direct part's checks (C#'s `mobileDirect.*` keys) from its report.
pub fn checks(report: &Value) -> Vec<(&'static str, bool)> {
    let yes = |key: &str| report.get(key) == Some(&Value::Bool(true));
    vec![
        ("mobileDirectRows", yes("rows")),
        ("mobileDirectStatus", yes("status")),
        ("mobileDirectFirewallRow", yes("firewallRow")),
        ("mobileDirectAddCommand", yes("addCommand")),
        ("mobileDirectRemoveCommand", yes("removeCommand")),
        ("mobileDirectOutcomes", yes("outcomes")),
        ("mobileDirectListener", yes("listener")),
        ("mobileDirectPassed", yes("passed") && report.get("error").is_none()),
    ]
}

fn require(holds: bool, why: impl Into<String>) -> Result<(), String> {
    if holds { Ok(()) } else { Err(why.into()) }
}

/// 모바일 원격 as Settings shows it, on a surface of the smoke's own.
struct MobileBox(Arc<Host>);

impl SurfaceModel for MobileBox {
    fn view(&mut self, ui: &mut Ui) -> View {
        let body = crate::settings::mobile::mobile_remote(&self.0, ui);
        View::Settings {
            title: String::new(),
            close: String::new(),
            nav: vec![],
            selected: "mobile".into(),
            sections: vec![Section { title: None, beta: false, node: Some(ui.section_body(body)) }],
            width: 1.0,
            height: 1.0,
        }
    }
}

fn objects<'a>(value: &'a Value, out: &mut Vec<&'a Value>) {
    match value {
        Value::Object(map) => {
            out.push(value);
            map.values().for_each(|v| objects(v, out));
        }
        Value::Array(items) => items.iter().for_each(|v| objects(v, out)),
        _ => {}
    }
}

/// The box read as its push, a fresh surface each time (as Settings reopened).
fn read(host: &Arc<Host>) -> Value {
    const VIEW: &str = "smoke-mobile-direct";
    let surfaces = Surfaces::new();
    surfaces.open(VIEW, Place::Surface, Box::new(MobileBox(host.clone())));
    surfaces.render(VIEW, &crate::native::env(host)).and_then(|push| serde_json::from_str(&push).ok()).unwrap_or(Value::Null)
}

/// The node shown with automation id `aid` (none when hidden: the page drops hidden rows).
fn node(read: &Value, aid: &str) -> Option<Value> {
    let mut all = vec![];
    objects(read, &mut all);
    all.into_iter().find(|n| n.get("aid").and_then(Value::as_str) == Some(aid)).cloned()
}

/// The words shown under `aid`.
fn words(read: &Value, aid: &str) -> Option<String> {
    let node = node(read, aid)?;
    let mut all = vec![];
    objects(&node, &mut all);
    Some(all.iter().filter_map(|n| n.get("text").and_then(Value::as_str)).collect::<Vec<_>>().join(""))
}

/// The first control of `aid` (the node itself or one inside it).
fn control(read: &Value, aid: &str) -> Option<Value> {
    let node = node(read, aid)?;
    let mut all = vec![];
    objects(&node, &mut all);
    all.into_iter().find(|n| n.get("id").is_some()).cloned()
}

/// A button's words: its label, or the words of what it shows.
fn label(read: &Value, aid: &str) -> Option<String> {
    let button = control(read, aid)?;
    if let Some(label) = button.get("label").and_then(Value::as_str) {
        return Some(label.to_string());
    }
    let mut all = vec![];
    objects(&button, &mut all);
    Some(all.iter().filter_map(|n| n.get("text").and_then(Value::as_str)).collect::<Vec<_>>().join(""))
}

fn disabled(control: &Value) -> bool {
    control.get("disabled") == Some(&Value::Bool(true))
}

async fn run(host: &Arc<Host>, checks: &mut Map<String, Value>) -> Result<(), String> {
    let held = host.view().snapshot.mobile_remote.clone();
    let recorder = Arc::new(RecordingNetsh::default());
    let fixed = MobileDirectStatus::new(
        DirectState::Listening,
        vec!["100.101.102.103:43139".into(), "[fd7a:115c:a1e0::1234:5678]:43139".into()],
        None,
    );
    crate::direct::smoke_use_firewall(Some(firewall(&recorder)), Some(fixed.clone()));
    let outcome = rows_and_rule(host, &recorder, &fixed, checks).await;
    crate::direct::smoke_use_firewall(None, None);
    let _ = host.view().update(|state| state.mobile_remote = held);
    outcome?;
    listener(host).await?;
    checks.insert("listener".into(), json!(true));
    Ok(())
}

/// The runner the smoke gives the window: C#'s, from the system folder's name (read, never run) for a fixed program, over the recorder.
fn firewall(recorder: &Arc<RecordingNetsh>) -> Arc<dyn mighty_platform::Firewall> {
    Arc::new(NetshRunner::new(recorder.clone(), Some(FakeFirewall::SYSTEM_DIRECTORY.into()), Some(FakeFirewall::PROGRAM.into()), None))
}

async fn rows_and_rule(
    host: &Arc<Host>,
    recorder: &Arc<RecordingNetsh>,
    fixed: &MobileDirectStatus,
    checks: &mut Map<String, Value>,
) -> Result<(), String> {
    let locale = host.locale;
    host.view().update(|state| {
        let mut remote = state.mobile_remote.clone().unwrap_or_default();
        remote.enabled = true;
        remote.direct_enabled = true;
        remote.direct_port = mighty_core::model::MOBILE_DIRECT_PORT;
        state.mobile_remote = Some(remote);
    })?;
    crate::direct::refresh_firewall_state(host).await;
    let shown = read(host);
    let toggle = control(&shown, "settings-mobile-direct-toggle").ok_or("the box has no settings-mobile-direct-toggle")?;
    require(toggle.get("on") == Some(&Value::Bool(true)), "the direct switch must start on")?;
    let port = control(&shown, "settings-mobile-direct-port").ok_or("the box has no settings-mobile-direct-port")?;
    require(port.get("text").and_then(Value::as_str) == Some("43139"), "the port field must show 43139")?;
    control(&shown, "settings-mobile-direct-apply").ok_or("the box has no settings-mobile-direct-apply")?;
    require(disabled(&toggle) && disabled(&port), "the switch and the port are off in a smoke run")?;
    checks.insert("rows".into(), json!(true));
    require(
        words(&shown, "settings-mobile-direct-status").as_deref() == Some(strings::status_detail(&locale, fixed).as_str()),
        "the status line must list the endpoints",
    )?;
    checks.insert("status".into(), json!(true));
    require(
        words(&shown, "settings-mobile-firewall-state").as_deref()
            == Some(strings::firewall_state(&locale, &FirewallRule::Missing).as_str()),
        format!("the rule must read as absent: {:?}", words(&shown, "settings-mobile-firewall-state")),
    )?;
    let button = control(&shown, "settings-mobile-firewall-button").ok_or("the box has no settings-mobile-firewall-button")?;
    require(
        label(&shown, "settings-mobile-firewall-button").as_deref()
            == Some(strings::firewall_button(&locale, &FirewallRule::Missing).as_str())
            && disabled(&button),
        "the rule's button must offer to add it and be off in a smoke run",
    )?;
    let launched = recorder.launched();
    require(launched.len() == 1 && !launched[0].elevated, "before a click only the unelevated query may run")?;
    checks.insert("firewallRow".into(), json!(true));

    let elevated = || recorder.launched().into_iter().filter(|launch| launch.elevated).collect::<Vec<_>>();
    let busy = || crate::direct::firewall_view(host).busy;
    let rule = || crate::direct::firewall_view(host).rule;
    let message = || crate::direct::firewall_view(host).message;
    crate::direct::change_firewall_rule(host, true).await;
    let adds = elevated();
    let expected =
        netsh::add_arguments(FakeFirewall::PROGRAM, &[i32::from(wire::DEFAULT_PORT)]).ok_or("the fixed program path was refused")?;
    require(
        adds.len() == 1
            && adds[0].exe == netsh::netsh(FakeFirewall::SYSTEM_DIRECTORY)
            && adds[0].command_line == netsh::command_text(&expected)
            && matches!(rule(), FirewallRule::Present { .. })
            && message().is_none()
            && !busy(),
        format!("add: {:?}", adds.iter().map(|l| format!("{} {}", l.exe, l.command_line)).collect::<Vec<_>>()),
    )?;
    checks.insert("addCommand".into(), json!(true));
    crate::direct::change_firewall_rule(host, false).await;
    let removes: Vec<_> = elevated().into_iter().skip(1).collect();
    require(
        removes.len() == 1
            && removes[0].command_line == netsh::command_text(&netsh::delete_arguments())
            && rule() == FirewallRule::Missing
            && !busy(),
        format!("remove: {:?}", removes.iter().map(|l| l.command_line.clone()).collect::<Vec<_>>()),
    )?;
    checks.insert("removeCommand".into(), json!(true));

    // A declined prompt and one Windows never answered: Core's words only, the rule as read again, the button free again.
    recorder.fail_elevated(Some(NetshFailure::Cancelled));
    crate::direct::change_firewall_rule(host, true).await;
    require(
        message() == strings::firewall_outcome(&locale, &FirewallChange::Declined) && rule() == FirewallRule::Missing && !busy(),
        format!("a declined prompt: {:?}", message()),
    )?;
    recorder.fail_elevated(Some(NetshFailure::TimedOut));
    crate::direct::change_firewall_rule(host, true).await;
    require(
        message() == strings::firewall_outcome(&locale, &FirewallChange::Failed("timeout".into()))
            && rule() == FirewallRule::Missing
            && !busy(),
        format!("a timed-out prompt: {:?}", message()),
    )?;
    // A rule that cannot be read: the button offers to read it again (unelevated), and that read clears the unknown state.
    recorder.fail_elevated(None);
    recorder.query_answers(Some(5));
    crate::direct::refresh_firewall_state(host).await;
    let shown = read(host);
    require(
        rule() == FirewallRule::Unreadable
            && label(&shown, "settings-mobile-firewall-button").as_deref() == Some(strings::firewall_recheck_button(&locale).as_str()),
        "an unreadable rule offers to check again",
    )?;
    recorder.query_answers(None);
    let before = elevated().len();
    crate::direct::recheck_firewall_rule(host).await;
    require(rule() == FirewallRule::Missing && !busy() && elevated().len() == before, "checking again reads the rule without elevating")?;
    checks.insert("outcomes".into(), json!(true));
    Ok(())
}

/// A stand-in for the relay host: admits every peer and keeps count of the sockets it was handed.
#[derive(Default)]
struct SmokeSink {
    admitted: AtomicUsize,
    accepted: AtomicUsize,
}

impl DirectSink for SmokeSink {
    fn admit(&self, _peer: IpAddr) -> bool {
        self.admitted.fetch_add(1, Ordering::SeqCst);
        true
    }
    fn accept(&self, socket: DirectSocket, _peer: SocketAddr) -> BoxFuture<'static, ()> {
        self.accepted.fetch_add(1, Ordering::SeqCst);
        Box::pin(async move { drop(socket) })
    }
    fn refused(&self, _peer: IpAddr) {}
}

/// The direct path on loopback port 0 through the fake network (which binds loopback alone): it listens and advertises the address
/// it bound, a phone's upgrade gets its 101 and the socket goes to the sink; the path then stops.
async fn listener(host: &Arc<Host>) -> Result<(), String> {
    let network = FakeNetwork::default();
    let log = {
        let host = Arc::downgrade(host);
        Arc::new(move |line: &str| {
            if let Some(host) = host.upgrade() {
                host.log.line(line);
            }
        })
    };
    let path =
        DirectPath::new(Arc::new(network.clone()), tauri::async_runtime::handle().inner().clone(), DirectPathOptions::default(), log);
    let sink = Arc::new(SmokeSink::default());
    path.set_sink(Some(sink.clone()));
    path.set_address_source(Some(Arc::new(|| Ok(vec!["127.0.0.1".to_string()]))), 0).await;
    path.apply(DirectSettings { enabled: true, direct_enabled: true, port: wire::DEFAULT_PORT }).await;
    let outcome = upgrade(&path, &network, &sink).await;
    path.dispose();
    outcome
}

async fn upgrade(path: &DirectPath, network: &FakeNetwork, sink: &SmokeSink) -> Result<(), String> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let endpoint = path.endpoints().pop().ok_or_else(|| format!("the path did not listen: {:?}", path.status()))?;
    require(endpoint.starts_with("127.0.0.1:") && path.status().listening(), format!("listening on loopback: {endpoint}"))?;
    require(
        network.binds().iter().all(|(address, _)| address.ip().is_loopback() && address.port() == 0),
        "only loopback port 0 was bound",
    )?;
    let mut phone = tokio::net::TcpStream::connect(&endpoint).await.map_err(|e| format!("the phone did not connect: {e}"))?;
    let request = format!(
        "GET /ws?v=1 HTTP/1.1\r\nHost: {endpoint}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\n\r\n"
    );
    phone.write_all(request.as_bytes()).await.map_err(|e| e.to_string())?;
    let mut answer = vec![0u8; 512];
    let read = tokio::time::timeout(Duration::from_secs(10), phone.read(&mut answer))
        .await
        .map_err(|_| "no 101 in 10 s")?
        .map_err(|e| e.to_string())?;
    let answer = String::from_utf8_lossy(&answer[..read]).into_owned();
    require(
        answer.starts_with("HTTP/1.1 101 Switching Protocols\r\n")
            && answer.contains("Sec-WebSocket-Accept: s3pPLMBiTxaQ9kYGzzhZRbK+xOo=\r\n"),
        format!("the upgrade's answer: {answer:?}"),
    )?;
    let handed = async {
        for _ in 0..200 {
            if sink.accepted.load(Ordering::SeqCst) == 1 {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        false
    };
    require(handed.await && sink.admitted.load(Ordering::SeqCst) == 1, "the upgraded socket went to the sink once")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// "mobile direct: the settings rows and the smoke are wired" (L499; D17): C# read the WinUI sources; the Rust form reads the
    /// equivalent Rust files — the rows draw every automation id, the box calls them, only Core's words reach the firewall row, a smoke
    /// never reads or changes this PC's rule, the button is busy while a change waits, the smoke step drives the rows under
    /// `mobileDirect` and the result carries its keys.
    #[test]
    fn mobile_direct_the_settings_rows_and_the_smoke_are_wired() {
        let rows = include_str!("settings/mobile_direct.rs");
        for id in [
            "settings-mobile-direct-toggle",
            "settings-mobile-direct-port",
            "settings-mobile-direct-apply",
            "settings-mobile-direct-status",
            "settings-mobile-firewall-state",
            "settings-mobile-firewall-button",
            "settings-mobile-firewall-update",
        ] {
            assert!(rows.contains(&format!("\"{id}\"")), "the mobile remote section draws {id}");
        }
        assert!(include_str!("settings/mobile.rs").contains("mobile_direct::rows(host, ui, &saved)"), "the box draws the direct rows");
        assert!(
            rows.contains("strings::description(") && rows.contains("strings::status_detail(") && rows.contains("strings::firewall_state("),
            "the rows say what Core words"
        );
        assert!(rows.contains("strings::firewall_recheck_button("), "an unknown rule can be checked again");
        let direct = include_str!("direct.rs");
        assert!(direct.contains("direct().message = strings::firewall_outcome(&host.locale, &result);"), "only Core's outcome words");
        assert!(direct.contains("return state.smoke_firewall.clone();"), "a smoke run never reads or changes this PC's firewall");
        assert!(direct.contains("if host.smoke.is_none() && !host.may_change_machine()"), "netsh runs only with --allow-state-write (D4)");
        assert!(
            direct.contains("state.busy = true;") && direct.contains("direct().busy = false;"),
            "the button is busy while a change waits"
        );
        assert!(rows.contains("!firewall.busy"), "the rows read the busy mark");
        assert!(direct.contains("switched_off(&host).await;"), "turning the path off takes the rule away from that same click");
        assert!(direct.contains("host.may_listen().then("), "the path exists only when the app may listen");
        let smoke = include_str!("smoke.rs");
        assert!(
            smoke.contains("(\"mobileDirect\", crate::smoke_direct::start, crate::smoke_direct::checks)"),
            "the remote step drives the rows"
        );
        assert!(
            KEYS.contains(&"removeCommand") && KEYS.contains(&"addCommand") && KEYS.contains(&"outcomes"),
            "the result carries C#'s keys"
        );
        let names: Vec<&str> = checks(&json!({})).into_iter().map(|(name, _)| name).collect();
        assert!(names.iter().all(|name| name.starts_with("mobileDirect")) && names.len() == KEYS.len() + 1, "one check per key and passed");
        let verb = ["run", "as"].concat();
        assert!(!rows.contains(&verb) && !direct.contains(&verb), "no window file names the elevation verb");
    }

    #[test]
    fn a_report_that_is_missing_fails_every_check() {
        assert!(checks(&json!({"error": "the remote step's mobileDirect part did not report"})).iter().all(|(_, passed)| !passed));
        let mut full: Map<String, Value> = KEYS.iter().map(|key| (key.to_string(), json!(true))).collect();
        full.insert("passed".into(), json!(true));
        assert!(checks(&Value::Object(full)).iter().all(|(_, passed)| *passed));
    }
}
