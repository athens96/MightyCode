//! MC-9: the control API end to end on the Mac — the real socket (the profile's `control/mighty.sock`), token and endpoint files of
//! a temporary profile, the real server and the CLI's own code — over a fake backend and bus instead of the window.

#![cfg(target_os = "macos")]

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use mighty_core::control::cli;
use mighty_core::control::events::{EventBody, PaneRef, PaneState, StateEvent, transition};
use mighty_core::control::protocol::{ControlError, ErrorCode, MAX_LINE_BYTES};
use mighty_platform::BoxFuture;
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::broadcast;

use super::bus::{Bus, BusEvent};
use super::server::{self, Backend};
use super::{client, endpoint};

/// What the fake app's quit takes down: its listener and endpoint (as `control::shutdown` does).
type Quit = (Arc<dyn mighty_platform::AgentIoListener>, PathBuf, String);

struct Fake {
    bus: Bus,
    states: Mutex<HashMap<String, PaneState>>,
    sent: Mutex<Vec<(String, String)>>,
    /// MC-3: it started without a window; its window shown since; the stops it was asked for; what its quit closes.
    headless: bool,
    shown: Mutex<bool>,
    stops: Mutex<Vec<bool>>,
    quit: Mutex<Option<Quit>>,
}

impl Fake {
    fn new() -> Arc<Fake> {
        Fake::started(false)
    }

    fn started(headless: bool) -> Arc<Fake> {
        let states = HashMap::from([("p1".to_string(), PaneState::Idle), ("p2".to_string(), PaneState::Running)]);
        Arc::new(Fake {
            bus: Bus::default(),
            states: Mutex::new(states),
            sent: Mutex::default(),
            headless,
            shown: Mutex::new(!headless),
            stops: Mutex::default(),
            quit: Mutex::default(),
        })
    }

    fn pane(&self, id: &str) -> Value {
        let state = self.states.lock().unwrap().get(id).copied().unwrap_or(PaneState::Idle);
        json!({ "id": id, "workspace": "w1", "kind": "agent", "agent": "claude", "state": state.name(), "attention": null,
                "worktree": { "branch": format!("mighty/{id}"), "portBlock": { "base": 41000, "count": 10 } } })
    }

    /// Moves `pane` and announces it on the bus, as the app's `touch` does.
    fn moves(&self, pane: &str, to: PaneState) {
        let from = self.states.lock().unwrap().insert(pane.into(), to).unwrap_or(PaneState::Idle);
        let reference = PaneRef { pane: pane.into(), workspace: "w1".into(), kind: "agent".into(), ..PaneRef::default() };
        self.bus.publish(reference, EventBody::State { from, to, event: transition(from, to), reason: None });
    }
}

impl Backend for Fake {
    fn status(&self) -> Result<Value, ControlError> {
        Ok(
            json!({ "app": { "name": "Mighty Code" }, "workspaces": [{ "id": "w1", "name": "app" }], "panes": [self.pane("p1"), self.pane("p2")] }),
        )
    }
    fn workspaces(&self) -> Result<Value, ControlError> {
        Ok(json!({ "workspaces": [{ "id": "w1", "name": "app", "path": "/tmp/app" }] }))
    }
    fn panes(&self, workspace: Option<&str>) -> Result<Value, ControlError> {
        match workspace {
            Some("w1") | None => Ok(json!({ "panes": [self.pane("p1"), self.pane("p2")] })),
            Some(_) => Err(ControlError::not_found("workspace")),
        }
    }
    fn read(&self, pane: &str, lines: usize) -> BoxFuture<'_, Result<Value, ControlError>> {
        let result = if pane == "p1" {
            let all = ["user: hi", "assistant: hello", "  [redacted: aws]"];
            let kept: Vec<&str> = all[all.len().saturating_sub(lines)..].to_vec();
            Ok(json!({ "pane": pane, "kind": "agent", "lines": kept, "truncated": lines < all.len() }))
        } else {
            Err(ControlError::not_found("pane"))
        };
        Box::pin(async move { result })
    }
    fn send(&self, pane: &str, text: &str) -> BoxFuture<'_, Result<Value, ControlError>> {
        self.sent.lock().unwrap().push((pane.into(), text.into()));
        let pane = pane.to_string();
        Box::pin(async move { Ok(json!({ "pane": pane, "outcome": "started", "state": "running" })) })
    }
    fn create(&self, workspace: &str, agent: &str, worktree: bool) -> BoxFuture<'_, Result<Value, ControlError>> {
        let value = json!({ "pane": { "id": "p3", "workspace": workspace, "agent": agent, "worktree": if worktree { json!({}) } else { Value::Null } } });
        Box::pin(async move { Ok(value) })
    }
    fn state(&self, pane: &str) -> Option<PaneState> {
        self.states.lock().unwrap().get(pane).copied()
    }
    fn subscribe(&self) -> broadcast::Receiver<BusEvent> {
        self.bus.subscribe()
    }
    fn redact(&self, text: &str) -> String {
        text.replace("sk-secret", "[redacted]")
    }
    fn server_status(&self) -> Result<Value, ControlError> {
        let shown = *self.shown.lock().unwrap();
        Ok(json!({ "running": true, "mode": if shown { "windowed" } else { "headless" }, "startedHeadless": self.headless,
                   "window": if shown { "shown" } else { "none" }, "version": "test", "pid": 4242, "uptimeSeconds": 1 }))
    }
    /// As the app does: answered first, the listener closed and the endpoint removed a moment later.
    fn server_stop(&self, stop_all: bool) -> Result<Value, ControlError> {
        self.stops.lock().unwrap().push(stop_all);
        let quit = self.quit.lock().unwrap().take();
        if let Some((listener, data, address)) = quit {
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(150)).await;
                listener.close();
                endpoint::remove_endpoint(&data, &address);
            });
        }
        Ok(json!({ "stopping": true, "pid": 4242, "keptDaemonWork": false }))
    }
    fn show_window(&self) -> Result<Value, ControlError> {
        let was = std::mem::replace(&mut *self.shown.lock().unwrap(), true);
        Ok(json!({ "window": if was { "shown" } else { "opening" }, "pid": 4242 }))
    }
    /// A headless fake pairs; one with its window refuses as an app without the SSH tunnel does.
    fn ssh_pair(&self) -> BoxFuture<'_, Result<Value, ControlError>> {
        let headless = self.headless;
        Box::pin(async move {
            if headless {
                Ok(json!({ "port": 50123, "code": SSH_CODE, "version": "test" }))
            } else {
                Err(ControlError::new(ErrorCode::Failed, "this app takes no SSH pairing"))
            }
        })
    }
}

const SSH_CODE: &str = "ssh-pairing-code-ssh-pairing-code-ssh-pairing";

/// A short profile folder under /tmp (a socket path must fit 104 bytes).
fn profile(name: &str) -> PathBuf {
    let folder = PathBuf::from(format!("/tmp/mc-ctl-{name}-{}", &uuid::Uuid::new_v4().simple().to_string()[..8]));
    std::fs::create_dir_all(&folder).unwrap();
    folder
}

struct Served {
    data: PathBuf,
    fake: Arc<Fake>,
    auth: Arc<server::Auth>,
    address: String,
    listener: Arc<dyn mighty_platform::AgentIoListener>,
}

impl Drop for Served {
    fn drop(&mut self) {
        self.listener.close();
        let _ = std::fs::remove_dir_all(&self.data);
    }
}

fn serve(name: &str) -> Served {
    serve_in(profile(name), Fake::new())
}

/// A fake app serving `data`'s control API.
fn serve_in(data: PathBuf, fake: Arc<Fake>) -> Served {
    let (listener, address, auth) = super::open(&data, "test").unwrap();
    *fake.quit.lock().unwrap() = Some((listener.clone(), data.clone(), address.clone()));
    let backend: Arc<dyn Backend> = fake.clone();
    tokio::spawn(server::serve(listener.clone(), backend, auth.clone()));
    Served { data, fake, auth, address, listener }
}

/// One CLI call: its exit code and what it printed (stdout parsed as JSON when it is one value).
async fn cli_call(data: &Path, args: &[&str]) -> (i32, String) {
    cli_call_with(data, args, String::new()).await
}

async fn cli_call_with(data: &Path, args: &[&str], stdin: String) -> (i32, String) {
    let args: Vec<String> = args.iter().map(|a| a.to_string()).collect();
    let invocation = cli::parse(&args).unwrap();
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let code = client::run_with(invocation, data, crate::platform::control_connector(), move || Ok(stdin), &mut out, &mut err).await;
    (code, String::from_utf8(out).unwrap())
}

fn parsed(text: &str) -> Value {
    serde_json::from_str(text).unwrap_or_else(|_| panic!("not JSON: {text}"))
}

#[tokio::test(flavor = "multi_thread")]
async fn the_cli_drives_the_app_over_its_private_socket() {
    use std::os::unix::fs::PermissionsExt as _;
    let served = serve("e2e");
    let data = served.data.as_path();
    let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
    assert_eq!(served.address, data.join("control").join("mighty.sock").to_string_lossy());
    assert_eq!(mode(Path::new(&served.address)), 0o600, "the socket is the user's alone");
    assert_eq!(mode(&data.join("control").join("token")), 0o600, "so is the token");
    assert_eq!(mode(&data.join("control")), 0o700);

    // Each command's JSON.
    let (code, out) = cli_call(data, &["status", "--json"]).await;
    assert_eq!(code, cli::EXIT_OK, "{out}");
    let status = parsed(&out);
    assert_eq!(status["panes"].as_array().unwrap().len(), 2);
    assert_eq!(status["panes"][0]["worktree"]["portBlock"]["count"], 10);
    assert_eq!(parsed(&cli_call(data, &["workspace", "list"]).await.1)["workspaces"][0]["id"], "w1");
    assert_eq!(parsed(&cli_call(data, &["pane", "list", "--workspace", "w1"]).await.1)["panes"][1]["state"], "running");
    let (code, out) = cli_call(data, &["pane", "list", "--workspace", "nope"]).await;
    assert_eq!((code, parsed(&out)["error"]["code"].as_str()), (cli::EXIT_ERROR, Some("not_found")));
    let read = parsed(&cli_call(data, &["pane", "read", "p1", "--lines", "2"]).await.1);
    assert_eq!(read["lines"], json!(["assistant: hello", "  [redacted: aws]"]));
    assert_eq!(read["truncated"], true);
    assert_eq!(cli_call(data, &["pane", "read", "p9"]).await.0, cli::EXIT_ERROR);
    let sent = parsed(&cli_call(data, &["agent", "send", "p1", "run", "the", "tests"]).await.1);
    assert_eq!((sent["outcome"].as_str(), sent["state"].as_str()), (Some("started"), Some("running")));
    let (code, _) = cli_call_with(data, &["agent", "send", "p1", "-"], "from stdin\n".into()).await;
    assert_eq!(code, cli::EXIT_OK);
    assert_eq!(
        *served.fake.sent.lock().unwrap(),
        vec![("p1".to_string(), "run the tests".to_string()), ("p1".to_string(), "from stdin\n".to_string())]
    );
    let made = parsed(&cli_call(data, &["agent", "new", "--workspace", "w1", "--agent", "codex", "--worktree"]).await.1);
    assert_eq!((made["pane"]["agent"].as_str(), made["pane"]["worktree"].is_object()), (Some("codex"), true));

    // Waits: at once when already there, a timeout's exit code, and the bus's event that ends one.
    let idle = parsed(&cli_call(data, &["agent", "wait", "p1", "--until", "idle"]).await.1);
    assert_eq!((idle["state"].as_str(), idle["until"].as_str()), (Some("idle"), Some("idle")));
    let started = std::time::Instant::now();
    let (code, out) = cli_call(data, &["agent", "wait", "p2", "--until", "done", "--timeout", "0.3"]).await;
    assert_eq!(code, cli::EXIT_TIMEOUT, "{out}");
    assert_eq!(parsed(&out)["error"]["code"], "timeout");
    assert!(started.elapsed() >= Duration::from_millis(300));
    let fake = served.fake.clone();
    let mover = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(200)).await;
        fake.moves("p2", PaneState::Blocked);
        tokio::time::sleep(Duration::from_millis(100)).await;
        fake.moves("p2", PaneState::Done);
    });
    let (code, out) = cli_call(data, &["agent", "wait", "p2", "--until", "done", "--timeout", "10"]).await;
    assert_eq!((code, parsed(&out)["state"].as_str()), (cli::EXIT_OK, Some("done")), "{out}");
    mover.await.unwrap();
    assert_eq!(cli_call(data, &["agent", "wait", "p9", "--until", "idle"]).await.0, cli::EXIT_ERROR, "no such pane");

    // Output waits match each new line, redacted; a pattern that does not read is refused.
    let fake = served.fake.clone();
    let writer = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(200)).await;
        fake.bus.output("p2", "build 1 of 3");
        fake.bus.output("p1", "tests: 12 passed sk-secret");
        fake.bus.output("p2", "compiling\ntests: 40 passed sk-secret\nmore");
    });
    let (code, out) = cli_call(data, &["pane", "wait-output", "p2", "--regex", r"tests: (\d+) passed", "--timeout", "10"]).await;
    assert_eq!(code, cli::EXIT_OK, "{out}");
    let found = parsed(&out);
    assert_eq!(found["line"], "tests: 40 passed [redacted]");
    assert_eq!(found["match"], "tests: 40 passed");
    writer.await.unwrap();
    assert_eq!(cli_call(data, &["pane", "wait-output", "p2", "--regex", "(", "--timeout", "1"]).await.0, cli::EXIT_ERROR);
    assert_eq!(cli_call(data, &["pane", "wait-output", "p2", "--regex", "never", "--timeout", "0.2"]).await.0, cli::EXIT_TIMEOUT);

    // The event stream: each admitted event on its own line, until the token is made new.
    let stream_data = data.to_path_buf();
    let streaming = tokio::spawn(async move { cli_call(&stream_data, &["events", "--pane", "p1"]).await });
    tokio::time::sleep(Duration::from_millis(300)).await;
    served.fake.moves("p2", PaneState::Running);
    served.fake.moves("p1", PaneState::Running);
    served.fake.bus.output("p1", "output is never streamed");
    served.fake.moves("p1", PaneState::Done);
    tokio::time::sleep(Duration::from_millis(200)).await;
    let fresh = endpoint::new_token();
    endpoint::write_token(data, &fresh).unwrap();
    served.auth.replace(fresh);
    let (code, out) = tokio::time::timeout(Duration::from_secs(5), streaming).await.unwrap().unwrap();
    assert_eq!(code, cli::EXIT_ERROR, "a new token ends the stream");
    let lines: Vec<Value> = out.lines().map(parsed).collect();
    let events: Vec<&Value> = lines.iter().filter(|l| l.get("type").is_some()).collect();
    assert_eq!(events.len(), 2, "{out}");
    assert_eq!((events[0]["event"].as_str(), events[1]["event"].as_str()), (Some(StateEvent::Started.name()), Some("done")));
    assert!(events.iter().all(|e| e["pane"] == "p1"));
    assert_eq!(lines.last().unwrap()["error"]["code"], "unauthorized");

    // The CLI reads the new token itself: it keeps working.
    assert_eq!(cli_call(data, &["status"]).await.0, cli::EXIT_OK);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_wrong_token_or_too_long_a_line_is_refused_and_the_connection_closed() {
    let served = serve("refuse");
    let data = served.data.as_path();
    // A token that is not the file's (another profile's, an old one).
    std::fs::write(data.join("control").join("token"), format!("{}\n", endpoint::new_token())).unwrap();
    let (code, out) = cli_call(data, &["status"]).await;
    assert_eq!((code, parsed(&out)["error"]["code"].as_str()), (cli::EXIT_ERROR, Some("unauthorized")));

    let raw = |line: Vec<u8>| {
        let address = served.address.clone();
        async move {
            let mut connection = crate::platform::control_connector().connect(&address, Duration::from_secs(3)).await.unwrap();
            // The server may close before it read everything (a line too long): what it answered is read all the same.
            let _ = connection.write_all(&line).await;
            // No more requests: a connection left open after an answer ends here.
            let _ = connection.shutdown().await;
            let mut answer = Vec::new();
            let _ = tokio::time::timeout(Duration::from_secs(5), connection.read_to_end(&mut answer)).await.unwrap();
            String::from_utf8(answer).unwrap()
        }
    };
    let answer = raw(b"{\"id\":7,\"cmd\":\"status\",\"token\":\"nope\"}\n{\"id\":8,\"cmd\":\"status\"}\n".to_vec()).await;
    assert_eq!(answer.lines().count(), 1, "closed after the refusal: {answer}");
    assert_eq!(parsed(answer.lines().next().unwrap())["error"]["code"], "unauthorized");
    let mut long = vec![b'x'; MAX_LINE_BYTES + 10];
    long.push(b'\n');
    let answer = raw(long).await;
    assert_eq!(parsed(answer.lines().next().unwrap())["error"]["code"], "too_large");
    let answer = raw(b"not json\n".to_vec()).await;
    assert_eq!(parsed(answer.lines().next().unwrap())["error"]["code"], "bad_request");
}

#[tokio::test(flavor = "multi_thread")]
async fn the_cli_says_when_the_app_is_not_running_and_a_second_app_does_not_take_over() {
    let data = profile("absent");
    let (code, out) = cli_call(&data, &["status"]).await;
    assert_eq!(code, cli::EXIT_ERROR);
    assert!(parsed(&out)["error"]["message"].as_str().unwrap().contains("not running"));
    let (first, address, _) = super::open(&data, "one").unwrap();
    assert!(super::open(&data, "two").is_err(), "a live socket is never taken over");
    assert_eq!(endpoint::read_endpoint(&data).unwrap().version, "one", "nor its endpoint");
    first.close();
    endpoint::remove_endpoint(&data, &address);
    assert!(endpoint::read_endpoint(&data).is_none());
    let (code, _) = cli_call(&data, &["status"]).await;
    assert_eq!(code, cli::EXIT_ERROR);
    let _ = std::fs::remove_dir_all(&data);
}

/// MC-3: `mighty server status|start|show|stop` against the profile's app: nothing running is told apart from an error, a start
/// launches once and waits for the app to answer (or times out), a second start finds it, show asks for its window and stop waits
/// until the app has quit.
#[tokio::test(flavor = "multi_thread")]
async fn the_server_commands_start_show_and_stop_the_app() {
    let data = profile("server");
    let (code, out) = cli_call(&data, &["server", "status"]).await;
    assert_eq!((code, parsed(&out)["running"].as_bool()), (cli::EXIT_ERROR, Some(false)), "{out}");
    let (code, out) = cli_call(&data, &["server", "stop"]).await;
    assert_eq!((code, parsed(&out)["stopped"].as_bool()), (cli::EXIT_OK, Some(false)), "stopping nothing is no error: {out}");

    // A launch that never answers times out (exit 2) and says which process.
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let code =
        client::server_start(&data, crate::platform::control_connector(), || Ok(77), Duration::from_millis(600), &mut out, &mut err).await;
    assert_eq!(code, cli::EXIT_TIMEOUT);
    assert!(parsed(&String::from_utf8(out).unwrap())["error"]["message"].as_str().unwrap().contains("pid 77"));

    // The launch starts the app (here a fake one serving the profile), the CLI waits for its answer.
    let served: Arc<Mutex<Option<Served>>> = Arc::default();
    let launched = served.clone();
    let folder = data.clone();
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let launch = move || {
        let slow = folder.clone();
        let into = launched.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(300)).await;
            *into.lock().unwrap() = Some(serve_in(slow, Fake::started(true)));
        });
        Ok(4242)
    };
    let code = client::server_start(&data, crate::platform::control_connector(), launch, Duration::from_secs(10), &mut out, &mut err).await;
    let started = parsed(&String::from_utf8(out).unwrap());
    assert_eq!(code, cli::EXIT_OK, "{started}");
    assert_eq!((started["started"].as_bool(), started["alreadyRunning"].as_bool()), (Some(true), Some(false)));
    assert_eq!((started["mode"].as_str(), started["startedHeadless"].as_bool()), (Some("headless"), Some(true)));

    // A second start finds it and launches nothing.
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let again = client::server_start(
        &data,
        crate::platform::control_connector(),
        || -> Result<u32, String> { panic!("a running app is never started twice") },
        Duration::from_secs(5),
        &mut out,
        &mut err,
    )
    .await;
    assert_eq!((again, parsed(&String::from_utf8(out).unwrap())["alreadyRunning"].as_bool()), (cli::EXIT_OK, Some(true)));

    let (code, out) = cli_call(&data, &["server", "status"]).await;
    assert_eq!((code, parsed(&out)["window"].as_str()), (cli::EXIT_OK, Some("none")));
    assert_eq!(parsed(&cli_call(&data, &["server", "show"]).await.1)["window"], "opening");
    assert_eq!(parsed(&cli_call(&data, &["server", "status"]).await.1)["mode"], "windowed");
    assert_eq!(parsed(&cli_call(&data, &["server", "show"]).await.1)["window"], "shown", "a shown window is only brought forward");

    // The stop: answered, then waited for until nobody answers and the process has ended.
    let fake = served.lock().unwrap().as_ref().unwrap().fake.clone();
    let waited: Arc<Mutex<Vec<u32>>> = Arc::default();
    let seen = waited.clone();
    let exited = move |pid: u32, _: Duration| {
        seen.lock().unwrap().push(pid);
        true
    };
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let code =
        client::server_stop(&data, crate::platform::control_connector(), true, Duration::from_secs(10), &exited, &mut out, &mut err).await;
    let stopped = parsed(&String::from_utf8(out).unwrap());
    assert_eq!((code, stopped["stopped"].as_bool(), stopped["pid"].as_u64()), (cli::EXIT_OK, Some(true), Some(4242)), "{stopped}");
    assert_eq!(*fake.stops.lock().unwrap(), vec![true], "--stop-all reached the app");
    assert_eq!(*waited.lock().unwrap(), vec![4242], "the process's end was waited for");
    assert!(endpoint::read_endpoint(&data).is_none());
    assert_eq!(cli_call(&data, &["server", "status"]).await.0, cli::EXIT_ERROR);
    drop(served.lock().unwrap().take());
}

/// MC-33: `mighty server ssh-pair` (what another computer's app runs here over ssh) prints the app's one-time code on standard output
/// only — the one channel back to that app — and nothing of it on standard error; an app that takes no pairing says so as an error.
#[tokio::test(flavor = "multi_thread")]
async fn the_ssh_pairing_code_goes_only_to_the_cli_caller() {
    let data = profile("sshpair");
    let served = serve_in(data.clone(), Fake::started(true));
    let args: Vec<String> = ["server", "ssh-pair"].iter().map(|a| a.to_string()).collect();
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let code =
        client::run_with(cli::parse(&args).unwrap(), &data, crate::platform::control_connector(), || Ok(String::new()), &mut out, &mut err)
            .await;
    assert_eq!(code, cli::EXIT_OK);
    let pairing = mighty_core::ssh::server::parse_pairing(&String::from_utf8(out).unwrap()).expect("the CLI prints the pairing");
    assert_eq!((pairing.port, pairing.code.as_str()), (50123, SSH_CODE));
    assert!(!String::from_utf8(err).unwrap().contains(SSH_CODE), "never on standard error");
    drop(served);

    let data = profile("sshpair-refused");
    let served = serve_in(data.clone(), Fake::started(false));
    let (code, out) = cli_call(&data, &["server", "ssh-pair"]).await;
    assert_eq!(code, cli::EXIT_ERROR);
    assert_eq!(parsed(&out)["error"]["message"], "this app takes no SSH pairing");
    drop(served);
}

/// MC-3: a start of a profile whose app runs without a window hands over to it (its window shown there) and ends; a `--server` start
/// ends when any app runs; a start beside an app with its own window goes on as before; nothing is asked of an empty profile.
#[test]
fn a_start_hands_over_to_the_running_server() {
    let options = |data: &Path, server: bool| crate::args::Options { profile: Some(data.to_path_buf()), server, ..Default::default() };
    let empty = profile("handoff-none");
    assert_eq!(crate::server::handoff::at_start(&options(&empty, false)), None);
    assert_eq!(crate::server::handoff::at_start(&options(&empty, true)), None);

    let headless = profile("handoff-server");
    let server = tauri::async_runtime::block_on(async { serve_in(headless.clone(), Fake::started(true)) });
    assert_eq!(crate::server::handoff::at_start(&options(&headless, true)), Some(0), "never a second server");
    assert!(!*server.fake.shown.lock().unwrap());
    assert_eq!(crate::server::handoff::at_start(&options(&headless, false)), Some(0), "the window is shown there");
    assert!(*server.fake.shown.lock().unwrap());
    let smoke = crate::args::Options { smoke_test: true, ..options(&headless, false) };
    assert_eq!(crate::server::handoff::at_start(&smoke), None, "a smoke never asks");

    let windowed = profile("handoff-window");
    let app = tauri::async_runtime::block_on(async { serve_in(windowed.clone(), Fake::started(false)) });
    assert_eq!(crate::server::handoff::at_start(&options(&windowed, false)), None, "as before the server existed");
    assert_eq!(crate::server::handoff::at_start(&options(&windowed, true)), Some(0), "a server never starts beside it");
    drop((server, app));
    let _ = std::fs::remove_dir_all(&empty);
}
