//! MC-34: the Seatbelt sandbox for real, on the Mac: profiles made by `sandbox::profile` run under `/usr/bin/sandbox-exec` and what
//! they let through is tried with real commands. Every folder is a check's own under the build's temporary folder (never `TMPDIR`,
//! which a profile opens, and never the person's home: `HOME` is a fake one). The agent CLIs installed here are started with
//! `--version` only (no paid run).
#![cfg(target_os = "macos")]
// The check's own probes (git to make the fixture repository, sandbox-exec around a shell or a CLI's --version): no child of the app.
#![allow(clippy::disallowed_methods)]

mod common;

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use mighty_core::model::{SandboxConfig, Workspace};
use mighty_core::permission::ToolPermissionRequest;
use mighty_platform::BoxFuture;
use mighty_run::manager::ResolveWorkspace;
use mighty_run::model::{CliCommand, RunEvent, StartRunRequest};
use mighty_run::sandbox::gate::{GateHooks, GatePolicy, NETWORK_TOOL, NetworkGate};
use mighty_run::sandbox::hosts::{AllowList, Target};
use mighty_run::sandbox::profile::{self, ProfileInput};
use mighty_run::sandbox::proxy::Proxy;
use mighty_run::sandbox::{self, Block, SandboxHost, SandboxSetup, reviewer};
use mighty_run::{RunManager, RunManagerOptions};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

fn ready() -> bool {
    let ok = sandbox::available();
    if !ok {
        eprintln!("sandbox-exec is missing: skipped");
    }
    ok
}

/// A check's own folder outside `TMPDIR`, canonical, with a fake home, a workspace (a git repository), an outside folder and a
/// temporary folder.
struct Place {
    root: PathBuf,
}

impl Place {
    fn new(name: &str) -> Place {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!(
            "sbx-{name}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::SeqCst)
        ));
        let _ = std::fs::remove_dir_all(&root);
        for dir in ["home/.ssh", "home/.aws", "home/.claude", "home/.codex", "ws/sub", "outside", "tmp", "data/agent-io", "profiles"] {
            std::fs::create_dir_all(root.join(dir)).unwrap();
        }
        let root = std::fs::canonicalize(&root).unwrap();
        std::fs::write(root.join("home/.ssh/id_ed25519"), "SECRET KEY").unwrap();
        std::fs::write(root.join("home/.aws/credentials"), "SECRET").unwrap();
        std::fs::write(root.join("home/.codex/auth.json"), "{\"token\":\"codex\"}").unwrap();
        std::fs::write(root.join("home/.claude/settings.json"), "{}").unwrap();
        std::fs::write(root.join("data/state.json"), "{\"secret\":1}").unwrap();
        std::fs::write(root.join("data/agent-io/mcp.json"), "{}").unwrap();
        std::fs::write(root.join("ws/AGENTS.md"), "rules\n").unwrap();
        std::fs::write(root.join("ws/sub/CLAUDE.md"), "rules\n").unwrap();
        std::fs::write(root.join("ws/.mighty.yaml"), "x: 1\n").unwrap();
        std::fs::write(root.join("ws/main.rs"), "fn main() {}\n").unwrap();
        let git = |args: &[&str]| {
            let status = Command::new("git").args(args).current_dir(root.join("ws")).env("HOME", root.join("home")).output().unwrap();
            assert!(status.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&status.stderr));
        };
        git(&["init", "-q", "-b", "main"]);
        git(&["-c", "user.name=t", "-c", "user.email=t@e", "-c", "commit.gpgsign=false", "add", "."]);
        git(&["-c", "user.name=t", "-c", "user.email=t@e", "-c", "commit.gpgsign=false", "commit", "-qm", "first"]);
        Place { root }
    }

    fn at(&self, path: &str) -> PathBuf {
        self.root.join(path)
    }

    fn input(&self, provider: &str, ports: Vec<u16>, limit_network: bool) -> ProfileInput {
        ProfileInput {
            provider: provider.into(),
            home: self.at("home"),
            folder: self.at("ws"),
            temp: self.at("tmp"),
            data_dir: Some(self.at("data")),
            state_overrides: vec![],
            sockets: vec![],
            ports,
            limit_network,
            protect_instructions: true,
            read_only: vec!["secrets".into()],
        }
    }

    fn profile(&self, input: &ProfileInput) -> PathBuf {
        let text = profile::generate(input).unwrap();
        let path = self.at(&format!("profiles/{}-{}.sb", input.provider, input.ports.len()));
        std::fs::write(&path, text).unwrap();
        path
    }

    /// `script` run by `/bin/sh` under `profile` in the workspace, with the fake home: (exit code, output and errors).
    fn sh(&self, profile: &Path, script: &str) -> (i32, String) {
        let output = Command::new(sandbox::SANDBOX_EXEC)
            .args(["-f", &profile.to_string_lossy(), "/bin/sh", "-c", script])
            .current_dir(self.at("ws"))
            .env("HOME", self.at("home"))
            .env("TMPDIR", self.at("tmp"))
            .env_remove("SSH_AUTH_SOCK")
            .output()
            .unwrap();
        let text = format!("{}{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
        (output.status.code().unwrap_or(-1), text)
    }

    fn ok(&self, profile: &Path, script: &str) {
        let (code, text) = self.sh(profile, script);
        assert_eq!(code, 0, "expected `{script}` to work: {text}");
    }

    fn refused(&self, profile: &Path, script: &str) {
        let (code, text) = self.sh(profile, script);
        assert_ne!(code, 0, "expected `{script}` to be refused: {text}");
    }
}

impl Drop for Place {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

/// Writes inside the workspace pass; outside it, into git's config and hooks and into the instruction files they fail; the secret
/// folders, another agent's sign-in and the app's data cannot be read; a commit still works.
#[test]
fn files_follow_the_profile() {
    if !ready() {
        return;
    }
    let place = Place::new("files");
    let profile = place.profile(&place.input("claude", vec![], true));
    place.ok(&profile, "touch inside.txt && echo more >> main.rs && mkdir -p new/dir && echo x > new/dir/f");
    place.ok(&profile, "echo temp > \"$TMPDIR/t\"");
    place.refused(&profile, &format!("touch '{}'", place.at("outside/x").display()));
    place.refused(&profile, &format!("touch '{}'", place.at("home/escape").display()));
    place.refused(&profile, "touch ../outside/y");
    place.refused(&profile, &format!("cat '{}'", place.at("home/.ssh/id_ed25519").display()));
    place.refused(&profile, &format!("ls '{}'", place.at("home/.ssh").display()));
    place.refused(&profile, &format!("cat '{}'", place.at("home/.aws/credentials").display()));
    place.refused(&profile, &format!("echo k >> '{}'", place.at("home/.ssh/authorized_keys").display()));
    place.refused(&profile, &format!("cat '{}'", place.at("home/.codex/auth.json").display()));
    place.ok(
        &profile,
        &format!("cat '{}' && touch '{}'", place.at("home/.claude/settings.json").display(), place.at("home/.claude/new").display()),
    );
    place.refused(&profile, &format!("cat '{}'", place.at("data/state.json").display()));
    place.ok(&profile, &format!("cat '{}'", place.at("data/agent-io/mcp.json").display()));
    place.refused(&profile, "echo evil > .git/hooks/pre-commit");
    place.refused(&profile, "echo '[core]' >> .git/config");
    place.refused(&profile, "echo x > .git/info/attributes");
    place.refused(&profile, "echo x >> AGENTS.md");
    place.refused(&profile, "echo x > agents.md");
    place.refused(&profile, "rm AGENTS.md");
    place.refused(&profile, "mv AGENTS.md kept.md");
    place.refused(&profile, "echo x >> sub/CLAUDE.md");
    place.refused(&profile, "echo x > .mighty.yaml");
    place.refused(&profile, "mkdir -p secrets && echo x > secrets/a");
    place.refused(&profile, &format!("ln -s '{}' link && echo x > link/z", place.at("outside").display()));
    place.ok(&profile, "git -c user.name=t -c user.email=t@e -c commit.gpgsign=false commit -qam work && git log --oneline | grep -q work");
    assert!(std::fs::read_to_string(place.at("ws/AGENTS.md")).unwrap() == "rules\n");
    assert!(!place.at("outside/x").exists());
}

/// A provider that keeps its sign-in in a file cannot read the login keychains; one that keeps it there can.
#[test]
fn the_keychain_is_open_only_to_the_agents_that_sign_in_there() {
    if !ready() {
        return;
    }
    let place = Place::new("keychain");
    std::fs::create_dir_all(place.at("home/Library/Keychains")).unwrap();
    std::fs::write(place.at("home/Library/Keychains/login.keychain-db"), "db").unwrap();
    let pi = place.profile(&place.input("pi", vec![], true));
    place.refused(&pi, "cat \"$HOME/Library/Keychains/login.keychain-db\"");
    let claude = place.profile(&place.input("claude", vec![], true));
    place.ok(&claude, "cat \"$HOME/Library/Keychains/login.keychain-db\"");
}

struct Recorder {
    cards: Mutex<Vec<ToolPermissionRequest>>,
    blocked: Mutex<Vec<Target>>,
}

fn recorder() -> (Arc<Recorder>, GateHooks) {
    let seen = Arc::new(Recorder { cards: Mutex::new(vec![]), blocked: Mutex::new(vec![]) });
    let (cards, blocked) = (seen.clone(), seen.clone());
    let hooks = GateHooks {
        card: Arc::new(move |c| cards.cards.lock().unwrap().push(c)),
        blocked: Arc::new(move |t, _| blocked.blocked.lock().unwrap().push(t.clone())),
    };
    (seen, hooks)
}

/// A local HTTP server answering `200 hello`.
async fn http_server() -> u16 {
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            tokio::spawn(async move {
                let mut buffer = [0u8; 4096];
                let _ = stream.read(&mut buffer).await;
                let _ = stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\nConnection: close\r\n\r\nhello").await;
            });
        }
    });
    port
}

/// Under the profile the run reaches the network only through its proxy: a listed target passes, the same server directly (curl
/// without the proxy, raw TCP) does not, and an unlisted one through the proxy is refused after its card.
#[tokio::test(flavor = "multi_thread")]
async fn the_network_goes_only_through_the_proxy() {
    if !ready() {
        return;
    }
    let place = Arc::new(Place::new("network"));
    let (listed, unlisted) = (http_server().await, http_server().await);
    let (seen, hooks) = recorder();
    let gate = NetworkGate::with_limit("pane", AllowList::new("", &[format!("127.0.0.1:{listed}")]), hooks, Duration::from_millis(300));
    let proxy = Proxy::start(Arc::new(GatePolicy(gate.clone()))).await.unwrap();
    let profile = place.profile(&place.input("codex", vec![proxy.port], true));
    let via = format!("http://127.0.0.1:{}", proxy.port);
    let run = |script: String| {
        let (place, profile) = (place.clone(), profile.clone());
        tokio::task::spawn_blocking(move || place.sh(&profile, &script))
    };
    let (code, text) = run(format!("curl -s -m 5 -x {via} http://127.0.0.1:{listed}/")).await.unwrap();
    assert_eq!((code, text.as_str()), (0, "hello"), "a listed target through the proxy");
    let (code, text) = run(format!("curl -s -m 5 --noproxy '*' http://127.0.0.1:{listed}/")).await.unwrap();
    assert_ne!(code, 0, "the same server directly: {text}");
    let (code, _) =
        run(format!("/usr/bin/python3 -I -c \"import socket; socket.create_connection(('127.0.0.1', {listed}), 3)\"")).await.unwrap();
    assert_ne!(code, 0, "raw TCP around the proxy");
    let (code, text) = run(format!("curl -s -m 5 -o /dev/null -w '%{{http_code}}' -x {via} http://127.0.0.1:{unlisted}/")).await.unwrap();
    assert_eq!((code, text.as_str()), (0, "403"), "an unlisted target is refused");
    let (code, text) = run(format!("curl -s -m 5 -o /dev/null -w '%{{http_code}}' -x {via} http://localhost:{listed}/")).await.unwrap();
    assert_eq!((code, text.as_str()), (0, "403"), "a name for a listed address is another target");
    let cards = seen.cards.lock().unwrap().clone();
    assert!(cards.iter().any(|c| c.tool_name == NETWORK_TOOL && c.summary == format!("127.0.0.1:{unlisted}") && c.state == "pending"));
    assert!(seen.blocked.lock().unwrap().contains(&Target { host: "127.0.0.1".into(), port: unlisted }));
    drop(proxy);
}

fn find(program: &str) -> Option<PathBuf> {
    std::env::var("PATH").ok()?.split(':').map(|d| Path::new(d).join(program)).find(|p| p.is_file())
}

/// The agent CLIs installed here start under their own profile (`--version` and `--help` only, fake home, no network): which do
/// is printed.
/// Hermes is left out: its launcher needs the person's real `~/.hermes` install (checked by hand, docs/sandbox.md).
#[test]
fn installed_agents_start_under_their_profile() {
    if !ready() {
        return;
    }
    let place = Place::new("agents");
    for (provider, program) in [("claude", "claude"), ("codex", "codex"), ("gemini", "agy"), ("opencode", "opencode"), ("pi", "pi")] {
        let Some(binary) = find(program) else {
            eprintln!("{program}: not installed, skipped");
            continue;
        };
        // As the run does before it starts the CLI ([`sandbox::prepare`]): its own folders exist.
        for folder in profile::state_folders(provider) {
            std::fs::create_dir_all(place.at("home").join(folder)).unwrap();
        }
        let profile = place.profile(&place.input(provider, vec![], true));
        for flag in ["--version", "--help"] {
            let output = Command::new(sandbox::SANDBOX_EXEC)
                .args(["-f", &profile.to_string_lossy()])
                .arg(&binary)
                .arg(flag)
                .current_dir(place.at("ws"))
                .env("HOME", place.at("home"))
                .env("TMPDIR", place.at("tmp"))
                .output()
                .unwrap();
            let text = String::from_utf8_lossy(&output.stdout);
            eprintln!("{program} {flag} under the sandbox: {:?} {}", output.status.code(), text.lines().next().unwrap_or("").trim());
            assert!(output.status.success(), "{program} {flag}: {}", String::from_utf8_lossy(&output.stderr));
        }
    }
}

/// The app's side of the sandbox in the end-to-end check: on for every run, the fake home, no reviewer.
struct TestHost {
    setup: SandboxSetup,
    blocked: Mutex<Vec<Block>>,
    /// What the reviewer answers at once (None: no reviewer can run).
    verdict: Option<reviewer::Verdict>,
    reviewed: Mutex<Vec<reviewer::Review>>,
}

impl SandboxHost for TestHost {
    fn setup(&self, _: &Workspace, _: &StartRunRequest) -> Option<SandboxSetup> {
        Some(self.setup.clone())
    }
    fn blocked(&self, _: &str, block: &Block) {
        self.blocked.lock().unwrap().push(block.clone());
    }
    fn review(&self, _: &SandboxConfig, review: reviewer::Review) -> Option<BoxFuture<'static, Result<reviewer::Opinion, String>>> {
        self.reviewed.lock().unwrap().push(review);
        let verdict = self.verdict?;
        Some(Box::pin(async move { Ok(reviewer::Opinion { verdict, reason: "looks fine".into() }) }))
    }
}

/// A whole run under the sandbox: a stand-in agent CLI (a `/bin/sh` script, as agy is in the runner checks) started by the run
/// engine writes in its folder, is refused outside it and its secrets, reaches a listed server through the proxy, raises a card for
/// an unlisted one (answered 거부 here) and ends; the card's refusal is a row in the pane and a block the host hears of.
#[tokio::test(flavor = "multi_thread")]
async fn a_fake_agent_run_works_under_the_sandbox_and_its_proxy() {
    if !ready() {
        return;
    }
    fake_agent_run(None).await;
}

/// The same run with the reviewer on, answering 허용 at once: a connection to an unlisted host is no safe category, so its allow
/// settles nothing (the person's 거부, a second later, does); the decision is a line in the pane.
#[tokio::test(flavor = "multi_thread")]
async fn a_reviewer_cannot_approve_what_is_not_safe() {
    if !ready() {
        return;
    }
    let (events, host) = fake_agent_run(Some(reviewer::Verdict::Allow)).await;
    let reviewed = host.reviewed.lock().unwrap().clone();
    assert!(reviewed.iter().any(|r| r.target.is_some() && r.task == "go"), "the reviewer saw the connection and the request");
    let lines: Vec<String> = events.iter().filter_map(|e| e.entry.as_ref()).map(|e| e.text().to_string()).collect();
    assert!(lines.iter().any(|l| l.contains("심사 에이전트가 판단을 맡겼습니다")), "{lines:?}");
}

async fn fake_agent_run(verdict: Option<reviewer::Verdict>) -> (Vec<RunEvent>, Arc<TestHost>) {
    use std::os::unix::fs::PermissionsExt as _;
    common::korean();
    let place = Place::new("e2e");
    let (listed, unlisted) = (http_server().await, http_server().await);
    let binary = place.at("bin/agy");
    std::fs::create_dir_all(place.at("bin")).unwrap();
    let script = format!(
        r#"#!/bin/sh
if [ "$1" = "--version" ]; then printf '0.20.0\n'; exit 0; fi
/bin/cat > /dev/null
log="$PWD/run.log"
touch "$PWD/inside.txt" && echo inside-ok >> "$log"
touch '{outside}' 2>/dev/null && echo outside-written >> "$log" || echo outside-refused >> "$log"
cat '{secret}' >/dev/null 2>&1 && echo secret-read >> "$log" || echo secret-refused >> "$log"
[ "$MIGHTY_SANDBOX" = 1 ] && echo marked >> "$log"
# The run's own proxy settings, without its loopback exception (both servers are on this computer).
/usr/bin/env -u NO_PROXY -u no_proxy /usr/bin/curl -s -m 10 "http://127.0.0.1:{listed}/" >> "$log"; echo >> "$log"
/usr/bin/curl -s -m 10 --noproxy '*' "http://127.0.0.1:{unlisted}/" >/dev/null 2>&1 && echo direct-reached >> "$log" || echo direct-refused >> "$log"
code=$(/usr/bin/env -u NO_PROXY -u no_proxy /usr/bin/curl -s -m 20 -o /dev/null -w '%{{http_code}}' "http://127.0.0.1:{unlisted}/"); echo "unlisted-$code" >> "$log"
printf '%s\n' '{{"event":"init","conversation_id":"fixture-session"}}'
exit 0
"#,
        outside = place.at("outside/agent.txt").display(),
        secret = place.at("home/.ssh/id_ed25519").display(),
    );
    std::fs::write(&binary, script).unwrap();
    std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o700)).unwrap();
    let command = CliCommand::new(binary.to_string_lossy().into_owned(), vec![], Some("0.20.0"));
    let catalog = common::catalog(move |_| Some(command.clone()));
    let host = Arc::new(TestHost {
        setup: SandboxSetup {
            config: SandboxConfig {
                enabled: true,
                allowed_hosts: vec![format!("127.0.0.1:{listed}")],
                reviewer: Some(verdict.is_some()),
                ..SandboxConfig::default()
            },
            home: place.at("home"),
            data_dir: Some(place.at("data")),
            profile_dir: place.at("profiles/runs"),
            windows: None,
        },
        blocked: Mutex::new(vec![]),
        verdict,
        reviewed: Mutex::new(vec![]),
    });
    let workspace = Workspace { path: Some(place.at("ws").to_string_lossy().into_owned()), ..Workspace::default() };
    let resolved = workspace.clone();
    let resolve: ResolveWorkspace = Arc::new(move |_| {
        let workspace = resolved.clone();
        Box::pin(async move { Ok(workspace) })
    });
    let events: Arc<Mutex<Vec<RunEvent>>> = Arc::default();
    let sink = events.clone();
    let mut options = RunManagerOptions::new("", resolve, Arc::new(move |event| sink.lock().unwrap().push(event)));
    options.sandbox = Some(host.clone());
    let answer: Arc<std::sync::OnceLock<std::sync::Weak<RunManager>>> = Arc::default();
    let answering = answer.clone();
    let cards: Arc<Mutex<Vec<ToolPermissionRequest>>> = Arc::default();
    let card_sink = cards.clone();
    options.permission_requested = Some(Arc::new(move |value: ToolPermissionRequest| {
        card_sink.lock().unwrap().push(value.clone());
        if value.state == "pending" && value.tool_name == NETWORK_TOOL {
            let manager = answering.get().and_then(std::sync::Weak::upgrade).unwrap();
            tokio::spawn(async move {
                // A moment after the card, so a reviewer that answers at once has had its turn.
                tokio::time::sleep(Duration::from_millis(1000)).await;
                manager.respond_to_tool_permission(&value.run_id, &value.id, false).unwrap();
            });
        }
    }));
    let manager = Arc::new(RunManager::new(catalog, options));
    answer.set(Arc::downgrade(&manager)).unwrap();
    manager.start(StartRunRequest::new("pane", workspace.id.as_deref().unwrap(), "claude", "go").with_provider("gemini")).await.unwrap();
    let ended = |events: &Arc<Mutex<Vec<RunEvent>>>| {
        events.lock().unwrap().iter().rev().find(|e| e.kind == "status").and_then(|e| e.status.clone()).filter(|s| s != "running")
    };
    common::until(|| ended(&events).is_some(), 60_000, "the run ends").await;
    let log = std::fs::read_to_string(place.at("ws/run.log")).unwrap_or_default();
    assert_eq!(ended(&events).as_deref(), Some("completed"), "{log}");
    let lines: Vec<&str> = log.lines().collect();
    assert_eq!(
        lines,
        ["inside-ok", "outside-refused", "secret-refused", "marked", "hello", "direct-refused", "unlisted-403"],
        "what the agent could do"
    );
    assert!(!place.at("outside/agent.txt").exists());
    let cards = cards.lock().unwrap().clone();
    let asked = cards.iter().find(|c| c.tool_name == NETWORK_TOOL && c.state == "pending").expect("a card for the unlisted host");
    assert_eq!(asked.summary, format!("127.0.0.1:{unlisted}"));
    assert!(cards.iter().any(|c| c.id == asked.id && c.state == "denied"));
    let blocked = host.blocked.lock().unwrap().clone();
    assert!(blocked.iter().any(|b| b.kind == "network" && b.what == format!("127.0.0.1:{unlisted}")));
    let rows: Vec<String> = events
        .lock()
        .unwrap()
        .iter()
        .filter_map(|e| e.entry.as_ref().and_then(|entry| entry.activity.clone()))
        .filter(|a| a.tool_name.as_deref() == Some("Sandbox"))
        .map(|a| a.summary)
        .collect();
    assert!(rows.iter().any(|r| r.contains(&format!("127.0.0.1:{unlisted}"))), "the refusal is a row in the pane: {rows:?}");
    assert!(std::fs::read_dir(place.at("profiles/runs")).map(|d| d.count()).unwrap_or(0) == 0, "the profile goes with the run");
    manager.dispose().await;
    let events = events.lock().unwrap().clone();
    (events, host)
}
