//! MC-33: the remote owner of an SSH machine's runs and terminals. A run of an SSH workspace is not started here: its CLI is started
//! on the far side by the system `ssh`, and what this computer owns is that `ssh` process — through the same keyed spawn as any run
//! (MC-3), so with the background daemon on, the daemon owns the `ssh` and the run keeps going after the app closes. The run engine
//! reads the CLI's JSON through ssh's pipes exactly as it reads a local CLI's.
//!
//! [`SshLaunch`] turns "this program with these arguments in the remote folder" into the local request: the `ssh` program, an argument
//! array (never a shell string here), the app's own local folder as the working folder, and only the few environment values that are
//! no secret ([`FORWARDED_ENV`]) passed to the far side. The machine's sign-in is OpenSSH's own (`~/.ssh/config`, ssh-agent): the
//! local environment ssh starts with keeps `SSH_AUTH_SOCK`.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use mighty_core::model::Workspace;
use mighty_core::ssh::command::{self, Shell};
use mighty_core::ssh::{Mux, Target};
use mighty_platform::{PtyRequest, PtySize, SpawnRequest};

/// The environment values a remote run gets: switches the run engine sets, none of them a secret. Everything else (the user's local
/// environment, tokens of the app's local bridges) stays here.
pub const FORWARDED_ENV: [&str; 3] = ["NO_COLOR", "CLAUDE_CODE_EMIT_SESSION_STATE_EVENTS", "CLAUDE_CODE_EFFORT_LEVEL"];

/// The providers whose CLI runs on an SSH machine in this version: the ones that speak over plain stdin and stdout. The ACP agents
/// (OpenCode, Hermes) are refused with a sentence.
pub fn supported_provider(provider: &str) -> bool {
    matches!(provider, "claude" | "codex" | "gemini" | "pi")
}

/// How to reach one SSH workspace's folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SshLaunch {
    /// The local `ssh` program (`/usr/bin/ssh`; Windows `…\OpenSSH\ssh.exe`).
    pub program: PathBuf,
    pub target: Target,
    pub mux: Mux,
    /// The remote folder.
    pub dir: String,
    /// The local folder ssh starts in (the workspace's empty stand-in).
    pub local: PathBuf,
}

/// Finds the launch of an SSH workspace (the app's machine list); Err with the sentence to show.
pub type SshResolver = Arc<dyn Fn(&Workspace) -> Result<SshLaunch, String> + Send + Sync>;

impl SshLaunch {
    /// A run of `program` (a bare name the far side finds on its login `PATH`) with `args`. `env` is the run's whole environment:
    /// only [`FORWARDED_ENV`] names reach the far side, and the local ssh keeps the rest (its agent socket, `HOME`, `PATH`).
    pub fn run_request(&self, program: &str, args: &[String], env: &BTreeMap<String, String>) -> Result<SpawnRequest, String> {
        let remote = command::sh(&self.run_script(program, args, env)?);
        Ok(self.request(command::run_args(&self.target, &self.mux, remote), env))
    }

    /// The far side's script of a run (what [`SshLaunch::run_request`] sends under `/bin/sh -c`; MC-40's held run starts it
    /// detached): only [`FORWARDED_ENV`] names of `env` go into it.
    pub fn run_script(&self, program: &str, args: &[String], env: &BTreeMap<String, String>) -> Result<String, String> {
        // MC-47: a pane worktree's own values (its PORT, port block and folders on the machine) go too; none is a secret.
        let forwarded: BTreeMap<String, String> = env
            .iter()
            .filter(|(name, _)| FORWARDED_ENV.contains(&name.as_str()) || crate::model::PANE_ENVIRONMENT_KEYS.contains(&name.as_str()))
            .map(|(n, v)| (n.clone(), v.clone()))
            .collect();
        command::run_script(&self.dir, program, args, &forwarded).map_err(|what| format!("ssh: invalid {what}"))
    }

    /// A shell pane's one-shot command, run by the far side's login shell in the folder.
    pub fn shell_request(&self, text: &str, env: &BTreeMap<String, String>) -> SpawnRequest {
        let remote = command::sh(&command::shell_command_script(&self.dir, text));
        self.request(command::run_args(&self.target, &self.mux, remote), env)
    }

    /// A short read-only remote command (a listing, a file read, a diff).
    pub fn exec_request(&self, remote: String, env: &BTreeMap<String, String>) -> SpawnRequest {
        self.request(command::exec_args(&self.target, &self.mux, remote), env)
    }

    /// A port forward's own ssh.
    pub fn forward_request(&self, local: u16, remote: u16, env: &BTreeMap<String, String>) -> SpawnRequest {
        self.request(command::forward_args(&self.target, &self.mux, local, remote), env)
    }

    /// Taking a forward back from the shared connection (None without ControlMaster).
    pub fn cancel_forward_request(&self, local: u16, remote: u16, env: &BTreeMap<String, String>) -> Option<SpawnRequest> {
        command::cancel_forward_args(&self.target, &self.mux, local, remote).map(|args| self.request(args, env))
    }

    /// A terminal pane: ssh in a local PTY, `-tt`, the login shell (or tmux) in the remote folder. `env` is the PTY's whole environment.
    pub fn pty_request(&self, shell: &Shell, env: BTreeMap<String, String>, size: PtySize) -> PtyRequest {
        PtyRequest {
            program: self.program.clone(),
            args: command::terminal_args(&self.target, &self.mux, &self.dir, shell),
            cwd: self.local.clone(),
            env,
            size,
        }
    }

    fn request(&self, args: Vec<String>, env: &BTreeMap<String, String>) -> SpawnRequest {
        let mut request = SpawnRequest::new(&self.program, args, &self.local);
        for (name, value) in env {
            request.env.insert(name.clone(), value.clone());
        }
        request
    }
}

// ── port forwards ────────────────────────────────────────────────────────────────────────────────────────────────────────

/// One open forward: this computer's `127.0.0.1:<local>` to the far side's `localhost:<remote>`, for a pane.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForwardInfo {
    pub pane: String,
    pub machine: String,
    pub remote: u16,
    pub local: u16,
}

struct Forward {
    id: u64,
    info: ForwardInfo,
    control: Arc<dyn mighty_platform::ChildControl>,
    launch: SshLaunch,
}

/// The open port forwards. Each is an `ssh -N -L` of its own (over the machine's shared connection where there is one); closing one
/// ends that ssh and, with ControlMaster, takes the forward back from the master too (a forward a multiplexed client asked for
/// belongs to the master and would outlive it). A forward whose ssh ends by itself (the connection went, the local port was taken)
/// is forgotten and `ended` is told.
#[derive(Default)]
pub struct Forwards {
    open: std::sync::Mutex<Vec<Forward>>,
    next: std::sync::atomic::AtomicU64,
}

impl Forwards {
    fn lock(&self) -> std::sync::MutexGuard<'_, Vec<Forward>> {
        self.open.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Opens `info`'s forward (nothing when that pane already forwards that remote port). `ended` runs once if its ssh ends by itself.
    pub async fn open(
        self: &Arc<Self>,
        processes: &dyn mighty_platform::ProcessHost,
        launch: SshLaunch,
        info: ForwardInfo,
        ended: Box<dyn FnOnce() + Send>,
    ) -> Result<(), String> {
        if self.lock().iter().any(|f| f.info.pane == info.pane && f.info.remote == info.remote) {
            return Ok(());
        }
        let child = processes.spawn(launch.forward_request(info.local, info.remote, &BTreeMap::new())).await.map_err(|e| e.to_string())?;
        let control = child.control();
        drop(child);
        let id = self.next.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.lock().push(Forward { id, info, control: control.clone(), launch });
        let forwards = Arc::downgrade(self);
        tokio::spawn(async move {
            let _ = control.wait().await;
            let Some(forwards) = forwards.upgrade() else { return };
            let mut open = forwards.lock();
            if let Some(at) = open.iter().position(|f| f.id == id) {
                open.remove(at);
                drop(open);
                ended();
            }
        });
        Ok(())
    }

    /// The open forwards, in the order they were opened.
    pub fn list(&self) -> Vec<ForwardInfo> {
        self.lock().iter().map(|f| f.info.clone()).collect()
    }

    /// Closes every forward `which` picks: its ssh ended, then (ControlMaster) `-O cancel` for the same `-L`, bounded.
    pub async fn close_where(&self, processes: &dyn mighty_platform::ProcessHost, which: impl Fn(&ForwardInfo) -> bool) -> usize {
        let taken: Vec<Forward> = {
            let mut open = self.lock();
            let (taken, kept): (Vec<Forward>, Vec<Forward>) = std::mem::take(&mut *open).into_iter().partition(|f| which(&f.info));
            *open = kept;
            taken
        };
        let count = taken.len();
        for forward in taken {
            let _ = forward.control.close().await;
            if let Some(cancel) = forward.launch.cancel_forward_request(forward.info.local, forward.info.remote, &BTreeMap::new()) {
                let _ = mighty_platform::process::capture(processes, cancel, None, std::time::Duration::from_secs(5), 64 * 1024).await;
            }
        }
        count
    }
}

/// `--plugin-dir <folder>` taken out of a Claude run's arguments: the app's Mods plugin is a local folder the far side does not have.
pub fn without_local_plugin(arguments: &mut Vec<String>) {
    while let Some(at) = arguments.iter().position(|a| a == "--plugin-dir") {
        let end = (at + 2).min(arguments.len());
        arguments.drain(at..end);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn launch() -> SshLaunch {
        SshLaunch {
            program: "/usr/bin/ssh".into(),
            target: Target::parse("devbox").unwrap(),
            mux: Mux::Shared("/data/ssh".into()),
            dir: "~/app".into(),
            local: "/data/ssh/workspaces/w1".into(),
        }
    }

    #[test]
    fn a_run_forwards_only_switches_that_are_no_secret() {
        let mut env = BTreeMap::new();
        env.insert("NO_COLOR".to_string(), "1".to_string());
        env.insert("ANTHROPIC_API_KEY".to_string(), "sk-secret".to_string());
        env.insert("MIGHTY_CLAUDE_BRIDGE_TOKEN".to_string(), "token".to_string());
        env.insert("SSH_AUTH_SOCK".to_string(), "/tmp/agent.sock".to_string());
        let request = launch().run_request("claude", &["--print".into()], &env).unwrap();
        assert_eq!(request.program, PathBuf::from("/usr/bin/ssh"));
        assert_eq!(request.cwd, PathBuf::from("/data/ssh/workspaces/w1"));
        let remote = command::unwrap_sh(request.args.last().unwrap()).unwrap();
        assert!(
            remote.contains("NO_COLOR=1") && !remote.contains("sk-secret") && !remote.contains("token") && !remote.contains("agent.sock")
        );
        assert_eq!(request.env.get("SSH_AUTH_SOCK").map(String::as_str), Some("/tmp/agent.sock"), "the local ssh keeps its agent");
        assert!(launch().run_request("../bin/claude", &[], &env).is_err());
    }

    /// MC-47: a pane worktree's run on the machine goes into its worktree with its PORT, port block and folders (none a secret).
    #[test]
    fn a_worktree_panes_run_takes_its_folder_and_values() {
        let mut env = BTreeMap::new();
        env.insert("PORT".to_string(), "3456".to_string());
        env.insert("MIGHTY_PORT".to_string(), "41010".to_string());
        env.insert("MIGHTY_WORKTREE".to_string(), "/home/me/.mightycode/worktrees/p1".to_string());
        env.insert("ANTHROPIC_API_KEY".to_string(), "sk-secret".to_string());
        let worktree = SshLaunch { dir: "/home/me/.mightycode/worktrees/p1".into(), ..launch() };
        let request = worktree.run_request("claude", &[], &env).unwrap();
        let remote = command::unwrap_sh(request.args.last().unwrap()).unwrap();
        assert!(remote.starts_with("cd '/home/me/.mightycode/worktrees/p1' || exit 1"), "{remote}");
        assert!(remote.contains("PORT=3456") && remote.contains("MIGHTY_PORT=41010") && remote.contains("MIGHTY_WORKTREE="));
        assert!(!remote.contains("sk-secret"));
    }

    #[test]
    fn the_local_plugin_folder_is_taken_out() {
        let mut args: Vec<String> = ["--print", "--plugin-dir", "/Users/me/plugin", "--verbose", "--plugin-dir"].map(String::from).to_vec();
        without_local_plugin(&mut args);
        assert_eq!(args, ["--print", "--verbose"]);
    }

    /// A forward is its own `ssh -N -L`; closing it ends that ssh and takes the forward back from the shared connection.
    #[tokio::test]
    async fn forwards_open_once_and_are_torn_down() {
        let processes = mighty_platform::fake::FakeProcessHost::default();
        processes.push(mighty_platform::fake::FakeScript { hang: true, ..Default::default() });
        let forwards = Arc::new(Forwards::default());
        let info = ForwardInfo { pane: "p1".into(), machine: "m1".into(), remote: 3000, local: 41000 };
        forwards.open(&processes, launch(), info.clone(), Box::new(|| {})).await.unwrap();
        forwards.open(&processes, launch(), info.clone(), Box::new(|| panic!("not opened twice"))).await.unwrap();
        assert_eq!(forwards.list(), std::slice::from_ref(&info));
        let spawned = processes.spawned();
        assert_eq!(spawned.len(), 1, "one ssh per forward");
        assert!(spawned[0].args.windows(2).any(|w| w == ["-L", "127.0.0.1:41000:localhost:3000"]));
        assert!(spawned[0].args.contains(&"-N".to_string()));

        assert_eq!(forwards.close_where(&processes, |f| f.pane == "p1").await, 1);
        assert!(forwards.list().is_empty());
        assert!(processes.closed(0), "the forward's ssh is ended");
        let spawned = processes.spawned();
        assert_eq!(spawned.len(), 2);
        assert!(spawned[1].args.windows(2).any(|w| w == ["-O", "cancel"]), "and the master lets the forward go");
        assert_eq!(forwards.close_where(&processes, |_| true).await, 0);
    }

    /// A forward whose ssh ends by itself (the connection went) is forgotten and told.
    #[tokio::test]
    async fn a_forward_that_ends_by_itself_is_forgotten() {
        let processes = mighty_platform::fake::FakeProcessHost::default();
        processes.push(mighty_platform::fake::FakeScript { exit_code: 255, ..Default::default() });
        let forwards = Arc::new(Forwards::default());
        let (sender, receiver) = tokio::sync::oneshot::channel();
        let info = ForwardInfo { pane: "p1".into(), machine: "m1".into(), remote: 8080, local: 41001 };
        forwards
            .open(
                &processes,
                launch(),
                info,
                Box::new(move || {
                    let _ = sender.send(());
                }),
            )
            .await
            .unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(5), receiver).await.unwrap().unwrap();
        assert!(forwards.list().is_empty());
    }

    #[test]
    fn a_terminal_is_ssh_in_a_pty() {
        let request = launch().pty_request(&Shell::Login, BTreeMap::new(), PtySize::default());
        assert_eq!(request.program, PathBuf::from("/usr/bin/ssh"));
        assert!(request.args.contains(&"-tt".to_string()));
        assert_eq!(command::unwrap_sh(request.args.last().unwrap()).unwrap(), "cd ~/'app' && exec \"${SHELL:-/bin/sh}\" -l");
        assert!(supported_provider("claude") && !supported_provider("opencode"));
    }
}
