//! MC-3 end to end: the built `MightyCode` started as `--daemon --profile <temporary folder>` (never the user's data folder, no window,
//! no webview), driven over its real socket with its real token. A stand-in CLI (`/bin/sh` running a short script that prints, reads
//! stdin, waits and prints again) runs as a kept agent run. The checks: one daemon per profile, private files, the stream, an app that
//! lets go and a later one that takes the stream up exactly where it stopped (what came meanwhile replayed once, then live output), a
//! client of another protocol refused, shutdown, and a daemon killed outright whose leftover child the next daemon ends.
#![cfg(target_os = "macos")]
#![allow(clippy::disallowed_methods, reason = "the test starts the built exe as the app starts its daemon, in a temporary profile")]

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use mighty_core::daemon::protocol::{Command as Request, PROTOCOL, SpawnSpec};
use mighty_run::daemon::client::{DaemonClient, ProcEvent};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const EXE: &str = env!("CARGO_BIN_EXE_MightyCode");
const WAIT: Duration = Duration::from_secs(10);

/// A short temporary profile (a socket path must fit `sun_path`), removed at the end.
fn profile(name: &str) -> PathBuf {
    let folder = PathBuf::from(format!("/tmp/mc-e2e-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&folder);
    std::fs::create_dir_all(&folder).unwrap();
    folder
}

fn start_daemon(profile: &Path) -> Child {
    Command::new(EXE)
        .args(["--daemon", "--profile"])
        .arg(profile)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap()
}

fn endpoint(profile: &Path) -> Option<Value> {
    serde_json::from_str(&std::fs::read_to_string(profile.join("daemon").join("endpoint.json")).ok()?).ok()
}

fn wait_for_endpoint(profile: &Path, pid: u32) -> Value {
    let deadline = Instant::now() + WAIT;
    loop {
        if let Some(endpoint) = endpoint(profile).filter(|e| e["pid"].as_u64() == Some(u64::from(pid))) {
            return endpoint;
        }
        assert!(Instant::now() < deadline, "the daemon did not write its endpoint");
        std::thread::sleep(Duration::from_millis(50));
    }
}

async fn connect(profile: &Path) -> Arc<DaemonClient> {
    let endpoint = endpoint(profile).unwrap();
    let token = std::fs::read_to_string(profile.join("daemon").join("token")).unwrap().trim().to_string();
    let connector = mighty_platform_macos::daemon_connector();
    DaemonClient::connect(connector.as_ref(), endpoint["address"].as_str().unwrap(), &token, "e2e", WAIT).await.unwrap()
}

fn mode(path: &Path) -> u32 {
    use std::os::unix::fs::PermissionsExt as _;
    std::fs::metadata(path).unwrap().permissions().mode() & 0o777
}

fn alive(pid: u32) -> bool {
    // SAFETY: signal 0 only asks whether the process exists.
    unsafe { libc_kill(pid as i32, 0) == 0 }
}

unsafe extern "C" {
    #[link_name = "kill"]
    fn libc_kill(pid: i32, signal: i32) -> i32;
}

fn script(profile: &Path, text: &str) -> SpawnSpec {
    SpawnSpec {
        program: "/bin/sh".into(),
        args: vec!["-c".into(), text.into()],
        cwd: profile.to_string_lossy().into_owned(),
        env: [("LANG".to_string(), "en_US.UTF-8".to_string())].into_iter().collect(),
        ..SpawnSpec::default()
    }
}

/// Output events until `until` is in what came (each byte once); the bytes and the offset just past them.
async fn read_until(events: &mut tokio::sync::mpsc::UnboundedReceiver<ProcEvent>, until: &str) -> (String, u64) {
    let mut seen = Vec::new();
    let mut end = 0;
    while !String::from_utf8_lossy(&seen).contains(until) {
        match tokio::time::timeout(WAIT, events.recv()).await.expect("output in time").expect("open") {
            ProcEvent::Output { offset, data, lost, .. } => {
                assert_eq!(lost, 0, "nothing lost");
                end = offset + data.len() as u64;
                seen.extend_from_slice(&data);
            }
            ProcEvent::Replayed => {}
            other => panic!("unexpected {other:?} before {until:?}"),
        }
    }
    (String::from_utf8(seen).unwrap(), end)
}

#[tokio::test]
async fn a_real_daemon_keeps_a_run_through_a_detach_and_hands_it_back_exactly_once() {
    let profile = profile("keep");
    let mut daemon = start_daemon(&profile);
    let first = wait_for_endpoint(&profile, daemon.id());
    assert_eq!((first["protocol"].as_u64(), first["transport"].as_str()), (Some(PROTOCOL), Some("unix")));
    let socket = PathBuf::from(first["address"].as_str().unwrap());
    assert_eq!(socket, profile.join("daemon").join("mighty.sock"));
    assert_eq!(mode(&socket), 0o600, "the socket is the user's alone");
    assert_eq!(mode(&profile.join("daemon")), 0o700);
    assert_eq!(mode(&profile.join("daemon").join("token")), 0o600);
    assert_eq!(mode(&profile.join("daemon").join("endpoint.json")), 0o600);
    // One daemon per profile: a second exits at once and leaves the first serving.
    let second = start_daemon(&profile).wait_with_output().unwrap();
    assert!(second.status.success());
    assert!(String::from_utf8_lossy(&second.stderr).contains("already running"), "{:?}", second.stderr);
    assert_eq!(endpoint(&profile).unwrap()["pid"], first["pid"]);

    // The first app: a kept run that prints, answers one line, then prints again a moment later and answers another.
    let app = connect(&profile).await;
    assert!(app.accepted());
    let spec = script(
        &profile,
        "printf 'hello 가나다\\n'; read a; printf 'got:%s\\n' \"$a\"; sleep 0.5; printf 'meanwhile 라마\\n'; read b; printf 'got:%s\\n' \"$b\"",
    );
    let started = app.request(Request::Spawn { key: Some("run:pane-e2e".into()), meta: json!({}), spec }).await.unwrap();
    let proc = started["proc"].as_str().unwrap().to_string();
    let (mut events, _) = app.attach(&proc, 0).await.unwrap();
    let (said, _) = read_until(&mut events, "hello 가나다\n").await;
    assert_eq!(said, "hello 가나다\n");
    assert!(app.send(Request::Write { proc: proc.clone(), data: b"one\n".to_vec() }));
    let (said, stopped_at) = read_until(&mut events, "got:one\n").await;
    assert_eq!(said, "got:one\n");
    // It quits keeping its work: where it stopped reading goes on the run, then it lets go and goes.
    app.request(Request::Meta { proc: proc.clone(), meta: json!({ "resumeAt": stopped_at }) }).await.unwrap();
    app.detach(&proc);
    drop(events);
    drop(app);
    // Meanwhile the run prints with nobody attached; the daemon keeps it.
    tokio::time::sleep(Duration::from_millis(900)).await;

    // A later app: the run is still there, and is taken up exactly where the first app stopped.
    let later = connect(&profile).await;
    let listed = later.list().await.unwrap();
    let info = listed.iter().find(|p| p.proc == proc).expect("the kept run is listed");
    assert!(info.running);
    assert_eq!(info.key.as_deref(), Some("run:pane-e2e"));
    assert_eq!(info.meta["resumeAt"].as_u64(), Some(stopped_at));
    let (mut events, _) = later.attach(&proc, stopped_at).await.unwrap();
    let mut replayed = Vec::new();
    loop {
        match tokio::time::timeout(WAIT, events.recv()).await.unwrap().unwrap() {
            ProcEvent::Output { offset, data, lost, .. } => {
                assert_eq!(lost, 0);
                assert_eq!(offset, stopped_at + replayed.len() as u64, "no gap, no repeat");
                replayed.extend_from_slice(&data);
            }
            ProcEvent::Replayed => break,
            other => panic!("unexpected {other:?}"),
        }
    }
    assert_eq!(String::from_utf8(replayed).unwrap(), "meanwhile 라마\n", "what came meanwhile, once, and nothing before it");
    assert!(later.send(Request::Write { proc: proc.clone(), data: "two 바\n".as_bytes().to_vec() }));
    let (said, _) = read_until(&mut events, "got:two 바\n").await;
    assert_eq!(said, "got:two 바\n", "live output follows");
    match tokio::time::timeout(WAIT, events.recv()).await.unwrap().unwrap() {
        ProcEvent::Exit { code, .. } => assert_eq!(code, Some(0)),
        other => panic!("unexpected {other:?}"),
    }
    later.request(Request::Release { proc: proc.clone() }).await.unwrap();
    assert!(later.list().await.unwrap().is_empty());

    // A client of another protocol is told the daemon's and refused anything but status and shutdown.
    let token = std::fs::read_to_string(profile.join("daemon").join("token")).unwrap().trim().to_string();
    let mut raw = tokio::net::UnixStream::connect(&socket).await.unwrap();
    let hello = Request::Hello { protocol: PROTOCOL + 1, app: "9.9.9".into(), token };
    raw.write_all(format!("{}\n{}\n", hello.line(1), Request::List.line(2)).as_bytes()).await.unwrap();
    let mut answer = String::new();
    let mut buffer = [0u8; 4096];
    while answer.matches('\n').count() < 2 {
        let read = tokio::time::timeout(WAIT, raw.read(&mut buffer)).await.unwrap().unwrap();
        assert!(read > 0);
        answer.push_str(&String::from_utf8_lossy(&buffer[..read]));
    }
    let lines: Vec<Value> = answer.lines().map(|l| serde_json::from_str(l).unwrap()).collect();
    assert_eq!((lines[0]["result"]["accepted"].as_bool(), lines[1]["ok"].as_bool()), (Some(false), Some(false)));
    drop(raw);

    // Shutdown: the daemon ends and its endpoint goes.
    let stopping = later.request(Request::Shutdown).await.unwrap();
    assert_eq!(stopping["stopping"], true);
    let deadline = Instant::now() + WAIT;
    while daemon.try_wait().unwrap().is_none() {
        assert!(Instant::now() < deadline, "the daemon did not exit");
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(daemon.wait().unwrap().success());
    assert!(endpoint(&profile).is_none(), "the endpoint file goes with the daemon");
    let _ = std::fs::remove_dir_all(&profile);
}

#[tokio::test]
async fn a_daemon_killed_outright_reads_as_gone_and_the_next_one_ends_what_it_left() {
    let profile = profile("death");
    let mut daemon = start_daemon(&profile);
    wait_for_endpoint(&profile, daemon.id());
    let app = connect(&profile).await;
    // A run that would go on for half a minute, reading nothing.
    let started = app
        .request(Request::Spawn { key: Some("run:pane-x".into()), meta: json!({}), spec: script(&profile, "exec sleep 30") })
        .await
        .unwrap();
    let proc = started["proc"].as_str().unwrap().to_string();
    let child = started["pid"].as_u64().unwrap() as u32;
    let (mut events, _) = app.attach(&proc, 0).await.unwrap();
    assert!(alive(child));
    // The daemon dies without its exit (SIGKILL): the app hears it, and the run reads as gone.
    daemon.kill().unwrap();
    let _ = daemon.wait();
    tokio::time::timeout(WAIT, app.closed()).await.expect("the app hears the daemon go");
    loop {
        match tokio::time::timeout(WAIT, events.recv()).await.unwrap() {
            Some(ProcEvent::Gone) | None => break,
            Some(_) => {}
        }
    }
    // The next daemon of the profile ends what the dead one left running.
    let mut next = start_daemon(&profile);
    wait_for_endpoint(&profile, next.id());
    let deadline = Instant::now() + WAIT;
    while alive(child) {
        assert!(Instant::now() < deadline, "the dead daemon's child was left running");
        std::thread::sleep(Duration::from_millis(50));
    }
    let later = connect(&profile).await;
    assert!(later.list().await.unwrap().is_empty(), "a new daemon starts with nothing");
    later.request(Request::Shutdown).await.unwrap();
    let _ = next.wait();
    let _ = std::fs::remove_dir_all(&profile);
}
