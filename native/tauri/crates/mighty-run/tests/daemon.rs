//! MC-3: the background daemon's engine, its client and the process-owner switch, over the in-memory transport and the platform's
//! fakes (no OS process; the real daemon process is src-tauri's `tests/daemon.rs`).

use std::sync::Arc;
use std::time::Duration;

use mighty_core::daemon::protocol::{Command, ErrorCode, Kind, MAX_LINE_BYTES, PROTOCOL};
use mighty_core::daemon::ring::Stream;
use mighty_platform::fake::{FakeAgentIo, FakeProcessHost, FakePtyHost, FakeScript};
use mighty_platform::{AgentIoConnector, AgentIoListener, ProcessHost, PtyHost, PtyRequest, PtySize, SpawnRequest};
use mighty_run::daemon::client::{DaemonClient, ProcEvent};
use mighty_run::daemon::owner::{DETACHED, RESUME_AT, Route, RoutedProcesses, RoutedPtys, parse_run_key, run_key};
use mighty_run::daemon::server::{Daemon, Options};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const TOKEN: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
const WAIT: Duration = Duration::from_secs(5);

struct Running {
    daemon: Arc<Daemon>,
    io: FakeAgentIo,
    name: String,
    processes: Arc<FakeProcessHost>,
    ptys: Arc<FakePtyHost>,
    served: tokio::task::JoinHandle<()>,
}

fn start(options: Options) -> Running {
    let processes = Arc::new(FakeProcessHost::default());
    let ptys = Arc::new(FakePtyHost::default());
    let daemon = Daemon::new(processes.clone(), ptys.clone(), options, Arc::new(|_: &str| {}));
    let io = FakeAgentIo::default();
    let name = io.bind().unwrap();
    let listener: Arc<dyn AgentIoListener> = Arc::new(io.clone());
    let served = tokio::spawn(daemon.clone().serve(listener));
    Running { daemon, io, name, processes, ptys, served }
}

impl Running {
    async fn client(&self) -> Arc<DaemonClient> {
        DaemonClient::connect(&self.io, &self.name, TOKEN, "0.6.0-test", WAIT).await.unwrap()
    }
}

async fn next(events: &mut tokio::sync::mpsc::UnboundedReceiver<ProcEvent>) -> ProcEvent {
    tokio::time::timeout(WAIT, events.recv()).await.expect("an event in time").expect("the stream is open")
}

async fn read_all(reader: &mut (dyn tokio::io::AsyncRead + Send + Unpin)) -> Vec<u8> {
    let mut out = Vec::new();
    tokio::time::timeout(WAIT, reader.read_to_end(&mut out)).await.expect("the reader ends").unwrap();
    out
}

#[tokio::test]
async fn hello_needs_the_token_and_another_protocol_may_only_see_and_stop() {
    let running = start(Options::new("0.6.0", TOKEN));
    // A wrong token is refused and the connection closed.
    let wrong = DaemonClient::connect(&running.io, &running.name, &"f".repeat(64), "x", WAIT).await.unwrap_err();
    assert_eq!(wrong.code, ErrorCode::Unauthorized);
    // A request before hello is refused too.
    let mut raw = running.io.connect(&running.name, WAIT).await.unwrap();
    raw.write_all(format!("{}\n", Command::List.line(1)).as_bytes()).await.unwrap();
    let mut answer = String::new();
    raw.read_to_string(&mut answer).await.unwrap();
    assert!(answer.contains("unauthorized"), "{answer}");
    // An app of another protocol: told the daemon's, not accepted, refused anything but status and shutdown.
    let connection = running.io.connect(&running.name, WAIT).await.unwrap();
    let (mut reader, mut writer) = tokio::io::split(connection);
    let hello = Command::Hello { protocol: PROTOCOL + 1, app: "9.9.9".into(), token: TOKEN.into() };
    writer.write_all(format!("{}\n{}\n{}\n", hello.line(1), Command::List.line(2), Command::Status.line(3)).as_bytes()).await.unwrap();
    let mut seen = String::new();
    let mut buffer = [0u8; 4096];
    while seen.matches('\n').count() < 3 {
        let read = tokio::time::timeout(WAIT, reader.read(&mut buffer)).await.unwrap().unwrap();
        assert!(read > 0);
        seen.push_str(&String::from_utf8_lossy(&buffer[..read]));
    }
    let lines: Vec<serde_json::Value> = seen.lines().map(|l| serde_json::from_str(l).unwrap()).collect();
    assert_eq!(lines[0]["result"]["accepted"], false);
    assert_eq!(lines[0]["result"]["protocol"], PROTOCOL);
    assert_eq!(lines[0]["result"]["app"], "0.6.0");
    assert_eq!(lines[1]["ok"], false, "list is refused to another protocol");
    assert_eq!(lines[2]["result"]["protocol"], PROTOCOL, "status is not");
    // The client side reads the same refusal as a client it may not drive.
    let client = running.client().await;
    assert!(client.accepted());
    running.daemon.stop();
    tokio::time::timeout(WAIT, running.served).await.unwrap().unwrap();
}

#[tokio::test]
async fn a_line_past_the_limit_closes_the_connection() {
    let running = start(Options::new("0.6.0", TOKEN));
    let mut raw = running.io.connect(&running.name, WAIT).await.unwrap();
    let hello = Command::Hello { protocol: PROTOCOL, app: "t".into(), token: TOKEN.into() };
    raw.write_all(format!("{}\n", hello.line(1)).as_bytes()).await.unwrap();
    let huge = vec![b'x'; MAX_LINE_BYTES + 10];
    let _ = raw.write_all(&huge).await;
    let mut answer = String::new();
    let _ = tokio::time::timeout(WAIT, raw.read_to_string(&mut answer)).await.unwrap();
    assert!(answer.contains("too_large"), "{answer}");
    running.daemon.stop();
}

#[tokio::test]
async fn a_run_streams_both_outputs_then_its_exit_and_is_released() {
    let running = start(Options::new("0.6.0", TOKEN));
    running.processes.push(FakeScript {
        stdout: b"{\"type\":\"result\"}\n".to_vec(),
        stderr: b"warn\n".to_vec(),
        exit_code: 3,
        hang: false,
    });
    let client = running.client().await;
    let route = Route::new();
    route.set(Some(client.clone()));
    let host = RoutedProcesses::new(running.processes.clone(), route.clone());
    let request = SpawnRequest::new("/bin/agent", ["-p", "hi"], "/tmp").env("SECRET", "kept-out-of-logs");
    let mut child = host.spawn_keyed(request, "pane-1").await.unwrap();
    let (mut out, mut err) = (child.stdout.take().unwrap(), child.stderr.take().unwrap());
    assert_eq!(read_all(&mut out).await, b"{\"type\":\"result\"}\n");
    assert_eq!(read_all(&mut err).await, b"warn\n");
    assert_eq!(tokio::time::timeout(WAIT, child.wait()).await.unwrap().unwrap(), 3);
    let spawned = running.processes.spawned();
    assert_eq!(spawned.len(), 1, "the daemon started it, through its own process host");
    assert_eq!(spawned[0].args, ["-p", "hi"]);
    assert_eq!(spawned[0].env.get("SECRET").map(String::as_str), Some("kept-out-of-logs"));
    // Read to its end, the app released it: the daemon forgets it.
    for _ in 0..50 {
        if client.list().await.unwrap().is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(client.list().await.unwrap().is_empty());
    running.daemon.stop();
}

#[tokio::test]
async fn stdin_writes_and_kills_go_to_the_daemon() {
    let running = start(Options::new("0.6.0", TOKEN));
    running.processes.push(FakeScript { hang: true, ..FakeScript::default() });
    let client = running.client().await;
    let route = Route::new();
    route.set(Some(client.clone()));
    let host = RoutedProcesses::new(running.processes.clone(), route.clone());
    let mut child = host.spawn_keyed(SpawnRequest::new("/bin/agent", ["x"], "/tmp"), "pane-1").await.unwrap();
    let mut stdin = child.stdin.take().unwrap();
    stdin.write_all("{\"type\":\"user\",\"text\":\"안녕\"}\n".as_bytes()).await.unwrap();
    for _ in 0..100 {
        if !running.processes.input(0).is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(String::from_utf8(running.processes.input(0)).unwrap(), "{\"type\":\"user\",\"text\":\"안녕\"}\n");
    let infos = client.list().await.unwrap();
    assert_eq!((infos.len(), infos[0].kind, infos[0].key.as_deref(), infos[0].running), (1, Kind::Run, Some("pane-1"), true));
    child.kill();
    assert_eq!(tokio::time::timeout(WAIT, child.wait()).await.unwrap().unwrap(), mighty_platform::fake::KILLED);
    running.daemon.stop();
}

fn pty_request() -> PtyRequest {
    PtyRequest {
        program: "/bin/zsh".into(),
        args: vec![],
        cwd: "/tmp".into(),
        env: std::collections::BTreeMap::new(),
        size: PtySize { columns: 100, rows: 30 },
    }
}

async fn read_text(output: &mut tokio::sync::mpsc::Receiver<Vec<u8>>, want: &str) -> String {
    let mut seen = Vec::new();
    while !String::from_utf8_lossy(&seen).contains(want) {
        let bytes = tokio::time::timeout(WAIT, output.recv()).await.expect("output in time").expect("open");
        seen.extend_from_slice(&bytes);
    }
    String::from_utf8(seen).unwrap()
}

#[tokio::test]
async fn a_terminal_kept_when_the_app_detaches_comes_back_with_its_output_exactly_once() {
    let running = start(Options::new("0.6.0", TOKEN));
    // The first app: a terminal pane's shell in the daemon (the fake terminal echoes its input).
    let first = running.client().await;
    let route = Route::new();
    route.set(Some(first.clone()));
    let ptys = RoutedPtys::new(running.ptys.clone(), route.clone());
    let mut session = ptys.open_keyed(pty_request(), "pane-7").await.unwrap();
    assert!(session.control.write("echo 가나\r".as_bytes()));
    assert_eq!(read_text(&mut session.output, "가나").await, "echo 가나\r");
    // It quits keeping its work: the close lets go at once, and the terminal still runs in the daemon.
    route.detach().await;
    tokio::time::timeout(Duration::from_secs(2), session.control.close()).await.expect("a detach does not wait").unwrap();
    assert_eq!(tokio::time::timeout(WAIT, session.control.wait()).await.unwrap().unwrap(), DETACHED);
    assert!(!running.ptys.closed(0), "the daemon's terminal was not closed");
    drop(first);
    // A later app: the daemon offers the pane's terminal, the pane gets it back with what it printed, once.
    let second = running.client().await;
    let infos = second.list().await.unwrap();
    assert_eq!((infos.len(), infos[0].key.as_deref(), infos[0].running), (1, Some("pane-7"), true));
    let route = Route::new();
    route.set(Some(second.clone()));
    route.offer(&infos);
    assert_eq!(route.offered_keys(), ["pane-7"]);
    let ptys = RoutedPtys::new(running.ptys.clone(), route.clone());
    let mut back = ptys.open_keyed(pty_request(), "pane-7").await.unwrap();
    assert_eq!(running.ptys.opened().len(), 1, "no new terminal opened");
    assert_eq!(read_text(&mut back.output, "가나").await, "echo 가나\r", "the scrollback replayed");
    assert!(back.control.write(b"ls\r"));
    assert_eq!(read_text(&mut back.output, "ls").await, "ls\r", "live output follows, nothing repeated");
    assert_eq!(running.ptys.sizes().last().copied(), Some(PtySize { columns: 100, rows: 30 }), "resized to the pane's grid");
    assert!(route.offered_keys().is_empty(), "given back once");
    // A pane that is not the terminal's opens a new one.
    let _other = ptys.open_keyed(pty_request(), "pane-8").await.unwrap();
    assert_eq!(running.ptys.opened().len(), 2);
    // Closing it now (not detaching) ends it in the daemon.
    back.control.close().await.unwrap();
    assert!(running.ptys.closed(0));
    running.daemon.stop();
}

#[tokio::test]
async fn a_small_ring_says_what_it_lost_and_replays_the_rest() {
    let mut options = Options::new("0.6.0", TOKEN);
    options.run_ring = 16;
    let running = start(options);
    running.processes.push(FakeScript { stdout: (0..100u8).map(|i| b'a' + i % 26).collect(), hang: true, ..FakeScript::default() });
    let client = running.client().await;
    let started = client
        .request(Command::Spawn {
            key: None,
            meta: serde_json::Value::Null,
            spec: mighty_core::daemon::protocol::SpawnSpec { program: "/bin/a".into(), cwd: "/tmp".into(), ..Default::default() },
        })
        .await
        .unwrap();
    let proc = started["proc"].as_str().unwrap().to_string();
    // Let the whole output reach the ring before attaching.
    for _ in 0..100 {
        if client.list().await.unwrap()[0].end == 100 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let (mut events, attached) = client.attach(&proc, 0).await.unwrap();
    assert_eq!((attached["start"].as_u64(), attached["end"].as_u64()), (Some(84), Some(100)));
    let ProcEvent::Output { stream, offset, data, lost } = next(&mut events).await else { panic!() };
    assert_eq!((stream, offset, lost, data.len()), (Stream::Out, 84, 84, 16));
    assert_eq!(next(&mut events).await, ProcEvent::Replayed);
    running.daemon.stop();
}

#[tokio::test]
async fn the_daemon_going_away_ends_every_remote_child_and_terminal() {
    // The daemon on a runtime of its own, so dropping that runtime is the daemon dying (every task and connection gone at once).
    let processes = Arc::new(FakeProcessHost::default());
    let ptys = Arc::new(FakePtyHost::default());
    processes.push(FakeScript { hang: true, ..FakeScript::default() });
    let io = FakeAgentIo::default();
    let name = io.bind().unwrap();
    let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(1).enable_all().build().unwrap();
    {
        let (processes, ptys, listener) = (processes.clone(), ptys.clone(), Arc::new(io.clone()) as Arc<dyn AgentIoListener>);
        runtime.spawn(async move {
            let daemon = Daemon::new(processes, ptys, Options::new("0.6.0", TOKEN), Arc::new(|_: &str| {}));
            daemon.serve(listener).await;
        });
    }
    let client = DaemonClient::connect(&io, &name, TOKEN, "t", WAIT).await.unwrap();
    let route = Route::new();
    route.set(Some(client.clone()));
    let host = RoutedProcesses::new(processes.clone(), route.clone());
    let terminals = RoutedPtys::new(ptys.clone(), route.clone());
    let child = host.spawn_keyed(SpawnRequest::new("/bin/agent", ["x"], "/tmp"), "pane-1").await.unwrap();
    let session = terminals.open_keyed(pty_request(), "pane-2").await.unwrap();
    runtime.shutdown_background();
    let failed = tokio::time::timeout(WAIT, child.wait()).await.expect("the wait ends").unwrap_err();
    assert!(failed.to_string().contains("daemon went away"), "{failed}");
    assert!(tokio::time::timeout(WAIT, session.control.wait()).await.unwrap().is_err(), "the terminal reads as ended");
    tokio::time::timeout(WAIT, client.closed()).await.unwrap();
    assert!(route.client().is_none(), "new work starts in-process");
    // ...and does: the next keyed spawn goes to the app's own host.
    processes.push(FakeScript::default());
    let _local = host.spawn_keyed(SpawnRequest::new("/bin/agent", ["y"], "/tmp"), "pane-3").await.unwrap();
    assert_eq!(processes.spawned().last().unwrap().args, ["y"]);
}

#[tokio::test]
async fn without_a_daemon_the_owner_is_the_app_as_before() {
    let processes = Arc::new(FakeProcessHost::default());
    let ptys = Arc::new(FakePtyHost::default());
    let route = Route::new();
    let host = RoutedProcesses::new(processes.clone(), route.clone());
    let terminals = RoutedPtys::new(ptys.clone(), route.clone());
    processes.push(FakeScript { stdout: b"ok\n".to_vec(), ..FakeScript::default() });
    let mut child = host.spawn_keyed(SpawnRequest::new("/bin/agent", ["a"], "/tmp"), "pane-1").await.unwrap();
    assert_eq!(read_all(child.stdout.as_mut().unwrap()).await, b"ok\n");
    assert_eq!(child.wait().await.unwrap(), 0);
    let _plain = host.spawn(SpawnRequest::new("/usr/bin/git", ["status"], "/tmp")).await.unwrap();
    assert_eq!(processes.spawned().len(), 2);
    let mut session = terminals.open_keyed(pty_request(), "pane-2").await.unwrap();
    assert!(session.control.write(b"x"));
    assert_eq!(session.output.recv().await.unwrap(), b"x");
    assert_eq!(ptys.opened().len(), 1);
    assert_eq!(terminals.default_shell(), ptys.default_shell());
    assert!(!route.connected() && route.client().is_none());
}

#[tokio::test]
async fn an_idle_daemon_exits_on_its_own() {
    let mut options = Options::new("0.6.0", TOKEN);
    options.idle = Some(Duration::from_millis(150));
    let running = start(options);
    // A client keeps it, so does a running process; with neither it goes.
    let client = running.client().await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(!running.served.is_finished(), "an attached app keeps it");
    running.processes.push(FakeScript { hang: true, ..FakeScript::default() });
    let route = Route::new();
    route.set(Some(client.clone()));
    let host = RoutedProcesses::new(running.processes.clone(), route.clone());
    let child = host.spawn_keyed(SpawnRequest::new("/bin/agent", ["x"], "/tmp"), &run_key("pane-1", true)).await.unwrap();
    route.detach().await;
    drop(child);
    drop(host);
    drop(route);
    drop(client);
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(!running.served.is_finished(), "a running process keeps it with nobody attached");
    assert_eq!(running.daemon.running(), 1);
    assert_eq!(running.daemon.clients(), 0);
    // The process ends (killed by another app): its unread end keeps the daemon waiting for the app that kept it.
    let killer = running.client().await;
    let proc = killer.list().await.unwrap()[0].proc.clone();
    killer.request(Command::Kill { proc: proc.clone() }).await.unwrap();
    for _ in 0..100 {
        if !killer.list().await.unwrap()[0].running {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    drop(killer);
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert!(!running.served.is_finished(), "an ended kept run nobody read holds the daemon longer");
    // Read (released) by an app, nothing is left, and the clock runs out.
    let reader = running.client().await;
    reader.request(Command::Release { proc }).await.unwrap();
    drop(reader);
    tokio::time::timeout(WAIT, running.served).await.expect("the daemon exits when idle").unwrap();
}

#[tokio::test]
async fn shutdown_ends_every_process_and_the_daemon() {
    let running = start(Options::new("0.6.0", TOKEN));
    running.processes.push(FakeScript { hang: true, ..FakeScript::default() });
    let client = running.client().await;
    let route = Route::new();
    route.set(Some(client.clone()));
    let host = RoutedProcesses::new(running.processes.clone(), route.clone());
    let _child = host.spawn_keyed(SpawnRequest::new("/bin/agent", ["x"], "/tmp"), "pane-1").await.unwrap();
    let status = client.status().await.unwrap();
    assert_eq!((status["running"].as_u64(), status["clients"].as_u64()), (Some(1), Some(1)));
    let stopping = client.request(Command::Shutdown).await.unwrap();
    assert_eq!(stopping["stopping"], true);
    tokio::time::timeout(WAIT, running.served).await.unwrap().unwrap();
    assert!(running.processes.closed(0), "the child was ended with the daemon");
}

#[tokio::test]
async fn a_detaching_app_keeps_runs_it_can_take_back_and_ends_the_rest() {
    let running = start(Options::new("0.6.0", TOKEN));
    // A kept run printed one whole line and half of the next before the app quit; a run that cannot be taken back runs beside it.
    running.processes.push(FakeScript { stdout: b"{\"type\":\"system\"}\n{\"type\":\"assi".to_vec(), hang: true, ..FakeScript::default() });
    running.processes.push(FakeScript { hang: true, ..FakeScript::default() });
    let client = running.client().await;
    let route = Route::new();
    route.set(Some(client.clone()));
    let host = RoutedProcesses::new(running.processes.clone(), route.clone());
    let mut kept = host.spawn_keyed(SpawnRequest::new("/bin/claude", ["x"], "/tmp"), &run_key("pane-1", true)).await.unwrap();
    let stopped = host.spawn_keyed(SpawnRequest::new("/bin/codex", ["y"], "/tmp"), &run_key("pane-2", false)).await.unwrap();
    let mut out = kept.stdout.take().unwrap();
    let mut first = [0u8; 64];
    let mut got = 0;
    while got < 31 {
        got += tokio::time::timeout(WAIT, out.read(&mut first[got..])).await.unwrap().unwrap();
    }
    route.detach().await;
    assert_eq!(tokio::time::timeout(WAIT, kept.wait()).await.unwrap().unwrap(), DETACHED);
    assert_eq!(tokio::time::timeout(WAIT, stopped.wait()).await.unwrap().unwrap(), DETACHED);
    let infos = running.client().await.list().await.unwrap();
    let kept_info = infos.iter().find(|i| i.key.as_deref() == Some("run:pane-1")).unwrap();
    assert!(kept_info.running, "the kept run still runs in the daemon");
    assert_eq!(kept_info.meta[RESUME_AT], 18, "a later app takes it up after the last whole line");
    for _ in 0..100 {
        if running.processes.closed(1)
            || !running.client().await.list().await.unwrap().iter().any(|i| i.key.as_deref() == Some("run-stop:pane-2") && i.running)
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(
        !running.client().await.list().await.unwrap().iter().any(|i| i.key.as_deref() == Some("run-stop:pane-2") && i.running),
        "a run that cannot be taken back was ended"
    );
    assert_eq!(parse_run_key("run:pane-1"), Some(("pane-1", true)));
    assert_eq!(parse_run_key("run-stop:pane-2"), Some(("pane-2", false)));
    assert_eq!(parse_run_key("pane-3"), None);
    // A later app takes the kept run back: its next keyed spawn attaches from there, starts nothing, and reads the half line whole.
    let later = running.client().await;
    let route = Route::new();
    route.set(Some(later.clone()));
    route.expect("run:pane-1", mighty_run::daemon::owner::Adoption { proc: kept_info.proc.clone(), pid: kept_info.pid, from: 18 });
    let host = RoutedProcesses::new(running.processes.clone(), route.clone());
    let mut back = host.spawn_keyed(SpawnRequest::new("/bin/claude", ["x"], "/tmp"), &run_key("pane-1", true)).await.unwrap();
    assert_eq!(running.processes.spawned().len(), 2, "nothing started again");
    let mut out = back.stdout.take().unwrap();
    let mut read = [0u8; 64];
    let n = tokio::time::timeout(WAIT, out.read(&mut read)).await.unwrap().unwrap();
    assert_eq!(&read[..n], b"{\"type\":\"assi", "from the line the earlier app did not finish");
    back.kill();
    assert_eq!(tokio::time::timeout(WAIT, back.wait()).await.unwrap().unwrap(), mighty_platform::fake::KILLED);
    running.daemon.stop();
}
