//! MC-33: an SSH workspace's runs through the remote owner, end to end over real children — never a real host. A stand-in `ssh`
//! (a `/bin/sh` script) takes the arguments the app gives the real one, records them, and runs the remote command here as the far
//! side's shell would; the far side's `$SHELL` is a stand-in too, and its `PATH` holds a `claude` that is the checks' fake CLI. The
//! local catalog finds no CLI at all: the run never needs one on this computer. As a real ssh, the far side starts with an
//! environment of its own (`env -i`), so a value reaches it only in the remote command.
#![cfg(unix)]

mod common;

use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock, Weak};

use mighty_core::model::{SshWorkspaceRef, Workspace};
use mighty_core::ssh::{Mux, Target};
use mighty_run::manager::{Emit, ResolveWorkspace};
use mighty_run::model::{RunEvent, StartRunRequest};
use mighty_run::permissions::ToolPermissionRequest;
use mighty_run::ssh::SshLaunch;
use mighty_run::{RunManager, RunManagerOptions};

#[derive(Clone, Default)]
struct Events(Arc<Mutex<Vec<RunEvent>>>);

impl Events {
    fn emit(&self) -> Emit {
        let list = self.0.clone();
        Arc::new(move |event| list.lock().unwrap().push(event))
    }
    fn of(&self, session: &str) -> Vec<RunEvent> {
        self.0.lock().unwrap().iter().filter(|e| e.session_id == session).cloned().collect()
    }
    fn last_status(&self, session: &str) -> Option<String> {
        self.of(session).iter().rev().find(|e| e.kind == "status").and_then(|e| e.status.clone())
    }
    fn text(&self, session: &str) -> String {
        format!("{:?}", self.of(session))
    }
}

fn script(path: &Path, text: &str) {
    std::fs::write(path, text).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).unwrap();
}

/// The stand-in machine: `ssh`, the far side's `$SHELL`, its `PATH` with `claude`, and a remote folder whose name needs quoting.
struct Machine {
    root: PathBuf,
    ssh: PathBuf,
    remote_dir: PathBuf,
    record: PathBuf,
    home: PathBuf,
}

impl Machine {
    fn new(name: &str) -> Machine {
        let root = common::temp(name);
        let bin = root.join("remote-bin");
        std::fs::create_dir_all(&bin).unwrap();
        let remote_dir = root.join("it's my \"app\"");
        std::fs::create_dir_all(&remote_dir).unwrap();
        let home = root.join("home");
        std::fs::create_dir_all(&home).unwrap();
        let record = root.join("claude");
        let shell = root.join("remote-shell");
        // `$SHELL -lc <command>` and `$SHELL -l`, as a login shell takes them, without reading any profile.
        script(&shell, "#!/bin/sh\nif [ \"$1\" = \"-lc\" ]; then shift; exec /bin/sh -c \"$1\"; fi\nexec /bin/sh \"$@\"\n");
        script(
            &bin.join("claude"),
            &format!(
                "#!/bin/sh\npwd > '{root}/cwd'\nprintf '%s' \"$NO_COLOR|$SECRET_TOKEN\" > '{root}/env'\nexec '{cli}' --fake-cli claude '{record}' \"$@\"\n",
                root = root.display(),
                cli = common::fake_text(),
                record = record.display()
            ),
        );
        let ssh = root.join("ssh");
        script(
            &ssh,
            &format!(
                "#!/bin/sh\nfor a in \"$@\"; do printf '%s\\n' \"$a\"; done > '{root}/ssh.args'\n\
                 while [ \"$#\" -gt 0 ]; do if [ \"$1\" = \"--\" ]; then shift; break; fi; shift; done\n\
                 shift\n\
                 if [ -f '{root}/lost' ]; then echo 'Connection to devbox closed by remote host.' >&2; exit 255; fi\n\
                 {decode}\n\
                 script=$(decode \"$1\")\n\
                 case \"$script\" in *'/Applications/MightyCode'*|*MightyCode-local*) echo 'the installed app is never run' >&2; exit 99;; esac\n\
                 case \"$script\" in *mighty-attach*) echo attach >> '{root}/attaches';; esac\n\
                 case \"$script\" in *mighty-attach*) if [ -f '{root}/cut-attaches' ]; then\n\
                 set -m\n\
                 /usr/bin/env -i SHELL='{shell}' PATH='{bin}':/usr/bin:/bin HOME='{home}' /bin/sh -c \"$1\" <&0 &\n\
                 p=$!\n\
                 while kill -0 \"$p\" 2>/dev/null; do\n\
                 if [ -f '{root}/cut' ]; then rm -f '{root}/cut'; kill -- -\"$p\" 2>/dev/null; echo 'Connection to devbox closed by remote host.' >&2; exit 255; fi\n\
                 sleep 0.2\n\
                 done\n\
                 wait \"$p\"; exit $?\n\
                 fi;; esac\n\
                 exec /usr/bin/env -i SHELL='{shell}' PATH='{bin}':/usr/bin:/bin HOME='{home}' /bin/sh -c \"$1\"\n",
                decode = common::DECODE_REMOTE,
                root = root.display(),
                shell = shell.display(),
                bin = bin.display(),
                home = home.display()
            ),
        );
        Machine { root, ssh, remote_dir, record, home }
    }

    fn workspace(&self) -> Workspace {
        let local = self.root.join("stand-in");
        std::fs::create_dir_all(&local).unwrap();
        Workspace {
            path: Some(local.to_string_lossy().into_owned()),
            ssh: Some(SshWorkspaceRef {
                machine: "0123456789abcdef0123456789abcdef".into(),
                dir: self.remote_dir.to_string_lossy().into_owned(),
                host: "devbox".into(),
            }),
            ..Workspace::default()
        }
    }

    fn options(&self, workspace: &Workspace, events: &Events) -> RunManagerOptions {
        let resolved = workspace.clone();
        let resolve: ResolveWorkspace = Arc::new(move |_| {
            let workspace = resolved.clone();
            Box::pin(async move { Ok(workspace) })
        });
        let mut options = RunManagerOptions::new("", resolve, events.emit());
        let (ssh, local) = (self.ssh.clone(), self.root.join("stand-in"));
        options.ssh = Some(Arc::new(move |workspace: &Workspace| {
            let reference = workspace.ssh.clone().ok_or("not ssh")?;
            Ok(SshLaunch {
                program: ssh.clone(),
                target: Target::parse("devbox").unwrap(),
                mux: Mux::Shared("/tmp/mc-ssh-test".into()),
                dir: reference.dir,
                local: local.clone(),
            })
        }));
        options
    }

    fn read(&self, name: &str) -> String {
        std::fs::read_to_string(self.root.join(name)).unwrap_or_default()
    }

    fn ssh_args(&self) -> Vec<String> {
        self.read("ssh.args").lines().map(str::to_string).collect()
    }
}

type Cell = Arc<OnceLock<Weak<RunManager>>>;

/// A Claude run with its approval channel: the CLI starts on the "far side" in the remote folder, the handshake and the prompt go
/// through ssh's pipes, the permission card reaches the host and its answer reaches the CLI.
#[tokio::test(flavor = "multi_thread")]
async fn a_claude_run_goes_through_ssh_with_its_approval_card() {
    common::korean();
    let machine = Machine::new("ssh-claude");
    let workspace = machine.workspace();
    let events = Events::default();
    let mut options = machine.options(&workspace, &events);
    let seen: Arc<Mutex<Vec<ToolPermissionRequest>>> = Arc::default();
    let cell: Cell = Arc::default();
    let (sink, answer) = (seen.clone(), cell.clone());
    options.permission_requested = Some(Arc::new(move |value: ToolPermissionRequest| {
        sink.lock().unwrap().push(value.clone());
        if value.state == "pending" {
            answer.get().and_then(Weak::upgrade).unwrap().respond_to_tool_permission("remote", &value.id, false).unwrap();
        }
    }));
    let manager = Arc::new(RunManager::new(common::absent_catalog(), options));
    cell.set(Arc::downgrade(&manager)).ok();
    // SAFETY: set before the run starts; the variable is the check's own and stays local.
    unsafe { std::env::set_var("SECRET_TOKEN", "must-not-travel") };
    manager.start(StartRunRequest::new("remote", workspace.id(), "claude", "fixture prompt")).await.unwrap();
    common::until(|| !manager.is_running("remote"), 20_000, "the run ends").await;
    manager.dispose().await;

    let ssh = machine.ssh_args();
    assert!(ssh.contains(&"-T".to_string()) && !ssh.contains(&"-tt".to_string()), "a run has no terminal: {ssh:?}");
    assert!(ssh.windows(2).any(|w| w == ["-o", "BatchMode=yes"]), "a run never prompts");
    assert!(ssh.windows(2).any(|w| w == ["-o", "ControlMaster=auto"]), "runs share the machine's connection");
    let at = ssh.iter().position(|a| a == "--").unwrap();
    assert_eq!(ssh[at + 1], "devbox");
    assert!(!ssh.last().unwrap().contains("must-not-travel"), "the local environment stays here");

    assert_eq!(machine.read("cwd").trim(), machine.remote_dir.to_string_lossy(), "the CLI works in the remote folder");
    assert_eq!(machine.read("env"), "1|", "only the run's switches reach the far side");
    let launched = common::recorded_args(&machine.record);
    assert_eq!(common::after(&launched, "--permission-prompt-tool"), "stdio");
    assert!(!launched.contains(&"--plugin-dir".to_string()), "the local plugin folder is not passed");
    let written = std::fs::read_to_string(format!("{}.input", machine.record.display())).unwrap();
    let lines: Vec<&str> = written.lines().filter(|l| !l.is_empty()).collect();
    assert_eq!(common::json(lines[0])["request"]["subtype"], "initialize", "the handshake went through ssh first");
    assert_eq!(common::json(lines[1])["type"], "user", "then the prompt");
    let seen = seen.lock().unwrap().clone();
    assert!(seen.first().is_some_and(|r| r.state == "pending" && r.tool_name == "Read"), "the card reached the host");
    assert_eq!(seen.last().unwrap().state, "denied");
    let decision = common::json(&std::fs::read_to_string(format!("{}.decision", machine.record.display())).unwrap());
    assert_eq!(decision["response"]["response"]["behavior"], "deny", "the answer reached the CLI through ssh");
    common::remove_when_free(&machine.root).await;
}

/// MC-40: the connection goes while the CLI waits on its approval card; the run goes on over there, the next attach reads on from
/// where this computer stopped, the answer given meanwhile reaches the CLI, and the run's folder is gone once it ended.
#[tokio::test(flavor = "multi_thread")]
async fn a_claude_run_survives_a_dropped_connection_with_its_card_waiting() {
    common::korean();
    let machine = Machine::new("ssh-held");
    std::fs::write(machine.root.join("cut-attaches"), "").unwrap();
    let workspace = machine.workspace();
    let events = Events::default();
    let mut options = machine.options(&workspace, &events);
    let seen: Arc<Mutex<Vec<ToolPermissionRequest>>> = Arc::default();
    let cell: Cell = Arc::default();
    let (sink, answer, root) = (seen.clone(), cell.clone(), machine.root.clone());
    options.permission_requested = Some(Arc::new(move |value: ToolPermissionRequest| {
        sink.lock().unwrap().push(value.clone());
        if value.state == "pending" {
            // The connection goes now; the person answers while it is away.
            std::fs::write(root.join("cut"), "").unwrap();
            let (answer, id) = (answer.clone(), value.id.clone());
            std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_millis(1500));
                answer.get().and_then(Weak::upgrade).unwrap().respond_to_tool_permission("remote", &id, false).unwrap();
            });
        }
    }));
    let manager = Arc::new(RunManager::new(common::absent_catalog(), options));
    cell.set(Arc::downgrade(&manager)).ok();
    manager.start(StartRunRequest::new("remote", workspace.id(), "claude", "fixture prompt")).await.unwrap();
    common::until(|| !manager.is_running("remote"), 60_000, "the run ends").await;
    manager.dispose().await;

    let attaches = std::fs::read_to_string(machine.root.join("attaches")).unwrap_or_default().lines().count();
    assert!(attaches >= 2, "the run was attached again after the connection went ({attaches})");
    let seen = seen.lock().unwrap().clone();
    assert!(seen.first().is_some_and(|r| r.state == "pending" && r.tool_name == "Read"), "the card reached the host");
    assert_eq!(seen.last().unwrap().state, "denied", "{seen:?}");
    let decision = common::json(&std::fs::read_to_string(format!("{}.decision", machine.record.display())).unwrap());
    assert_eq!(decision["response"]["response"]["behavior"], "deny", "the answer reached the CLI after the reconnect");
    assert_ne!(events.last_status("remote").as_deref(), Some("error"), "{}", events.text("remote"));
    let runs = machine.home.join(".cache/mighty-code/runs");
    let left: Vec<_> = std::fs::read_dir(&runs).map(|r| r.flatten().collect()).unwrap_or_default();
    assert!(left.is_empty(), "the ended run's folder is removed: {left:?}");
    common::remove_when_free(&machine.root).await;
}

/// A shell pane's command runs in the remote folder; a lost connection (ssh's 255) ends the run as an error.
#[tokio::test(flavor = "multi_thread")]
async fn a_shell_command_runs_remotely_and_a_lost_connection_is_an_error() {
    common::korean();
    let machine = Machine::new("ssh-shell");
    let workspace = machine.workspace();
    let events = Events::default();
    let manager = RunManager::new(common::absent_catalog(), machine.options(&workspace, &events));
    manager.start(StartRunRequest::new("shell", workspace.id(), "shell", "printf 'here:%s' \"$(basename \"$PWD\")\"")).await.unwrap();
    common::until(|| events.last_status("shell").is_some_and(|s| s != "running"), 20_000, "the command ends").await;
    assert_eq!(events.last_status("shell").as_deref(), Some("completed"), "{}", events.text("shell"));
    assert!(events.text("shell").contains("here:it's my \\\"app\\\""), "{}", events.text("shell"));

    std::fs::write(machine.root.join("lost"), "").unwrap();
    manager.start(StartRunRequest::new("lost", workspace.id(), "shell", "true")).await.unwrap();
    common::until(|| events.last_status("lost").is_some_and(|s| s != "running"), 20_000, "the run ends").await;
    assert_eq!(events.last_status("lost").as_deref(), Some("error"), "{}", events.text("lost"));
    manager.dispose().await;
    common::remove_when_free(&machine.root).await;
}

/// An SSH workspace's run is refused when the app gave no way to reach the machine, and an ACP agent is refused with a sentence.
#[tokio::test(flavor = "multi_thread")]
async fn runs_the_remote_owner_cannot_carry_are_refused() {
    common::korean();
    let machine = Machine::new("ssh-refused");
    let workspace = machine.workspace();
    let events = Events::default();
    let mut options = machine.options(&workspace, &events);
    options.ssh = None;
    let manager = RunManager::new(common::absent_catalog(), options);
    manager.start(StartRunRequest::new("none", workspace.id(), "claude", "hi")).await.unwrap();
    common::until(|| events.last_status("none").is_some_and(|s| s != "running"), 20_000, "the run ends").await;
    assert_eq!(events.last_status("none").as_deref(), Some("error"));
    manager.dispose().await;

    let events = Events::default();
    let manager = RunManager::new(common::absent_catalog(), machine.options(&workspace, &events));
    manager.start(StartRunRequest::new("acp", workspace.id(), "claude", "hi").with_provider("opencode")).await.unwrap();
    common::until(|| events.last_status("acp").is_some_and(|s| s != "running"), 20_000, "the run ends").await;
    assert_eq!(events.last_status("acp").as_deref(), Some("error"));
    assert!(machine.ssh_args().is_empty(), "nothing was started");
    manager.dispose().await;
    common::remove_when_free(&machine.root).await;
}
