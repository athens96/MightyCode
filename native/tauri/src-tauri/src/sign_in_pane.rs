//! Mac stage M-J, `PlatformProfile::agent_sign_in` = `TerminalPane`: a CLI's sign-in opens as a shell pane of a local workspace with
//! its login command typed and run there (Swift `AppStore.startCLILogin`, AppStore+CLIAccounts.swift:66-101), where Windows opens a
//! console window of its own (the platform's `TerminalLauncher`). It is the accounts coordinator's `TerminalLauncher` on the Mac
//! (accounts.rs `sign_in_launcher`), so the sign-in's flow (the status read until it says signed in) is the same code.

use std::sync::Arc;

use mighty_platform::{AgentSignIn, BoxFuture, PlatformError, TerminalLaunch, TerminalLauncher, TerminalWindow};
use tauri::Manager;

use crate::host::Host;

/// Where a sign-in opens: the platform's terminal window (Windows, and any smoke, whose launcher records instead of opening), or a
/// pane of the window (the Mac).
pub fn sign_in_launcher(
    sign_in: AgentSignIn,
    smoke: bool,
    window: Arc<dyn TerminalLauncher>,
    pane: impl FnOnce() -> Arc<dyn TerminalLauncher>,
) -> Arc<dyn TerminalLauncher> {
    match sign_in {
        AgentSignIn::TerminalPane if !smoke => pane(),
        _ => window,
    }
}

/// A word as a POSIX shell reads it back unchanged: as it is when it is plain, else in single quotes.
fn quoted(word: &str) -> String {
    // `=`, `%` and `~` are plain inside a word only: first, they would make an assignment, a job or a home folder of it.
    let plain = !word.is_empty()
        && !word.starts_with(['=', '%', '~'])
        && word.chars().all(|c| c.is_ascii_alphanumeric() || "_./:@%+=,-~".contains(c));
    if plain { word.to_string() } else { format!("'{}'", word.replace('\'', r"'\''")) }
}

/// The line typed into the pane (Swift `CLIAccountSupport.loginCommand`): the command and its arguments, quoted for the shell, with
/// Bedrock's provider switch in front of its setup (`CLAUDE_CODE_USE_BEDROCK=1 claude /setup-bedrock`). None for no command.
pub fn command_line(launch: &TerminalLaunch) -> Option<String> {
    if launch.argv.first().is_none_or(|first| first.is_empty()) {
        return None;
    }
    let line = launch.argv.iter().map(|word| quoted(word)).collect::<Vec<_>>().join(" ");
    let bedrock = launch.argv.iter().any(|a| a == "/setup-bedrock")
        && launch.environment.get("CLAUDE_CODE_USE_BEDROCK").is_some_and(|value| value == "1");
    Some(if bedrock { format!("CLAUDE_CODE_USE_BEDROCK=1 {line}") } else { line })
}

/// The pane's title: the caller's, else Swift's ("{provider} sign-in", Bedrock's setup its own).
fn title(launch: &TerminalLaunch, locale: &mighty_core::locale::Locale) -> String {
    if let Some(title) = &launch.title {
        return title.clone();
    }
    if launch.argv.iter().any(|a| a == "/setup-bedrock") {
        return locale.get("settings.cliAccounts.bedrockTerminalTitle");
    }
    let program =
        launch.argv.first().map(|first| std::path::Path::new(first).file_stem().unwrap_or_default().to_string_lossy().into_owned());
    let provider = match program.as_deref() {
        Some("agy") => "gemini",
        Some(name) => name,
        None => "claude",
    };
    let label = mighty_updates::cli_update::CliUpdateService::provider_label(provider);
    locale.get_with("loginRecovery.terminalTitle", &[("provider", label)])
}

/// The Mac's sign-in "window": a shell pane of the window.
pub struct PaneSignIn {
    pub app: tauri::AppHandle,
}

/// The pane holds no process of the launcher's: the sign-in is seen by its status, as a Windows Terminal tab's is.
struct Opened;

impl TerminalWindow for Opened {
    fn holds_command(&self) -> bool {
        false
    }
    fn wait(&self) -> BoxFuture<'_, mighty_platform::Result<i32>> {
        Box::pin(async { Ok(0) })
    }
}

/// Where the sign-in pane goes: the active workspace when it is on this computer, else the first that is (Swift `startCLILogin`);
/// None when there is none.
pub fn local_workspace(snapshot: &mighty_core::AppSnapshot) -> Option<String> {
    let local = |id: &str| snapshot.workspace(id).is_some_and(|w| w.remote.is_none());
    let active = snapshot.active_workspace_id.clone().filter(|id| local(id));
    active.or_else(|| snapshot.workspaces().iter().find(|w| w.remote.is_none()).map(|w| w.id().to_string()))
}

impl TerminalLauncher for PaneSignIn {
    /// Settings and the setup screen close (Swift closes its sheet so the pane can be added); the chosen local workspace becomes the
    /// active one and gets the shell pane now, so a refusal (no local workspace, the 128-pane limit) is the caller's error; then the
    /// command is typed into it once its shell is up (styles.rs `type_into_pane`, Swift's `autoRun`).
    fn open(&self, launch: TerminalLaunch) -> mighty_platform::Result<Box<dyn TerminalWindow>> {
        let Some(host) = self.app.try_state::<Arc<Host>>().map(|host| host.inner().clone()) else {
            return Err(PlatformError::unsupported("terminal pane", "open"));
        };
        let command = command_line(&launch).ok_or_else(|| PlatformError::Invalid("no command".into()))?;
        let refused = |key: &str| PlatformError::Refused(host.locale.get(key));
        let workspace = {
            let mut view = host.view();
            let workspace = local_workspace(&view.snapshot).ok_or_else(|| refused("settings.cliAccounts.noWorkspace"))?;
            if !mighty_core::add_pane::has_room(&view.snapshot) {
                return Err(refused("settings.cliAccounts.tooManyPanes"));
            }
            if view.snapshot.active_workspace_id.as_deref() != Some(workspace.as_str()) {
                crate::frame::select_workspace(&mut view, &workspace).map_err(PlatformError::Refused)?;
            }
            workspace
        };
        crate::settings::close(&host);
        crate::native::close(&host, mighty_tools::view::SETUP_SURFACE);
        let pane_id = mighty_core::wire::new_id();
        let (made, named) = (pane_id.clone(), title(&launch, &host.locale));
        let added = crate::frame::add_pane_to(&host, Some(&workspace), "shell", "claude", None, move |pane| {
            mighty_core::model::RunSession { id: Some(made), title: Some(named), ..pane }
        });
        match added {
            Ok(true) => {}
            Ok(false) => return Err(refused("settings.cliAccounts.terminalFailed")),
            Err(error) => return Err(PlatformError::Refused(error)),
        }
        host.schedule();
        tauri::async_runtime::spawn(async move {
            if let Err(error) = crate::styles::type_into_pane(&host, &pane_id, &command, true).await {
                host.log.line(&format!("sign-in pane: {error}"));
            }
        });
        Ok(Box::new(Opened))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mighty_platform::fake::RecordingTerminalLauncher;
    use std::collections::BTreeMap;

    /// `PlatformProfile::agent_sign_in` over both values: Windows' console window, the Mac's pane; a smoke keeps its recording window.
    #[test]
    fn the_sign_in_opens_where_the_profile_says() {
        let window: Arc<dyn TerminalLauncher> = Arc::new(RecordingTerminalLauncher::default());
        let pane: Arc<dyn TerminalLauncher> = Arc::new(RecordingTerminalLauncher::default());
        let windows = sign_in_launcher(mighty_platform::PlatformProfile::windows().agent_sign_in, false, window.clone(), || pane.clone());
        let mac = sign_in_launcher(mighty_platform::PlatformProfile::macos().agent_sign_in, false, window.clone(), || pane.clone());
        let smoke = sign_in_launcher(AgentSignIn::TerminalPane, true, window.clone(), || pane.clone());
        assert!(Arc::ptr_eq(&windows, &window) && Arc::ptr_eq(&mac, &pane) && Arc::ptr_eq(&smoke, &window));
    }

    /// The typed line is Swift's login command, each word read back as given by the shell; Bedrock's setup carries its switch.
    #[test]
    fn the_typed_line_is_the_login_command() {
        let launch = |argv: &[&str], environment: &[(&str, &str)]| TerminalLaunch {
            argv: argv.iter().map(|a| a.to_string()).collect(),
            environment: environment.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect::<BTreeMap<_, _>>(),
            ..TerminalLaunch::default()
        };
        assert_eq!(command_line(&launch(&["claude", "auth", "login"], &[])).as_deref(), Some("claude auth login"));
        assert_eq!(
            command_line(&launch(&["/Users/me/.local/bin/claude", "/setup-bedrock"], &[("CLAUDE_CODE_USE_BEDROCK", "1")])).as_deref(),
            Some("CLAUDE_CODE_USE_BEDROCK=1 /Users/me/.local/bin/claude /setup-bedrock")
        );
        assert_eq!(command_line(&launch(&["/Users/me/my tools/agy", "it's"], &[])).as_deref(), Some(r"'/Users/me/my tools/agy' 'it'\''s'"));
        assert_eq!(command_line(&launch(&["agy", "~x", "=y", "%1", "a=b"], &[])).as_deref(), Some("agy '~x' '=y' '%1' a=b"));
        assert_eq!(command_line(&launch(&[], &[])), None);
        let l = mighty_core::locale::Locale::new("en");
        assert_eq!(title(&launch(&["/opt/bin/agy"], &[]), &l), "Antigravity sign-in");
        assert_eq!(title(&launch(&["claude", "/setup-bedrock"], &[]), &l), "Claude Bedrock setup");
    }

    /// Mac integration review 16: the typed line through M-A's real PTY host, as the sign-in pane types it into a zsh pane (Swift
    /// `startCLILogin` runs its `loginCommand` there): every word arrives as given, the quoted ones included.
    #[cfg(target_os = "macos")]
    #[tokio::test(flavor = "multi_thread")]
    async fn the_typed_line_reaches_a_real_shell_word_for_word() {
        let words = ["it's", "~x", "=y", "%1", "a b", "a=b"];
        let mut argv = vec!["/usr/bin/printf".to_string(), "[%s]".to_string()];
        argv.extend(words.iter().map(|w| w.to_string()));
        let line = command_line(&TerminalLaunch { argv, ..TerminalLaunch::default() }).unwrap();
        let host = crate::platform::pty_host();
        let shell = std::path::PathBuf::from("/bin/zsh");
        let request = mighty_platform::PtyRequest {
            args: host.shell_arguments(&shell, None, true),
            program: shell,
            cwd: std::fs::canonicalize(std::env::temp_dir()).unwrap(),
            env: BTreeMap::from([("PATH".to_string(), "/usr/bin:/bin".to_string()), ("TERM".to_string(), "xterm-256color".to_string())]),
            size: mighty_platform::PtySize { columns: 200, rows: 24 },
        };
        let mut session = host.open(request).await.unwrap();
        assert!(session.control.write(format!("{line}\r").as_bytes()));
        let expected: String = words.iter().map(|w| format!("[{w}]")).collect();
        let mut seen = String::new();
        let until = std::time::Instant::now() + std::time::Duration::from_secs(15);
        // The terminal's echo of the typed line holds the words quoted; only the command's own output holds them bracketed.
        while !seen.contains(&expected) && std::time::Instant::now() < until {
            match tokio::time::timeout(std::time::Duration::from_millis(500), session.output.recv()).await {
                Ok(Some(chunk)) => seen.push_str(&String::from_utf8_lossy(&chunk)),
                Ok(None) => break,
                Err(_) => {}
            }
        }
        session.control.close().await.unwrap();
        assert!(seen.contains(&expected), "every word as given: {seen:?}");
    }

    /// The pane goes to the active workspace when it is local, else the first local one; none when every workspace is remote.
    #[test]
    fn the_pane_goes_to_a_local_workspace() {
        use mighty_core::model::{RemoteWorkspaceReference, Workspace};
        let remote = |id: &str| Workspace {
            id: Some(id.into()),
            remote: Some(RemoteWorkspaceReference {
                connection_id: Some("c".into()),
                workspace_id: Some("w".into()),
                host_name: None,
                version: 2,
                mirror: false,
            }),
            ..Workspace::default()
        };
        let local = |id: &str| Workspace { id: Some(id.into()), path: Some(format!("/w/{id}")), ..Workspace::default() };
        let state = |workspaces: Vec<Workspace>, active: &str| mighty_core::AppSnapshot {
            workspaces: Some(workspaces),
            active_workspace_id: Some(active.into()),
            ..mighty_core::AppSnapshot::default()
        };
        assert_eq!(local_workspace(&state(vec![local("a"), local("b")], "b")).as_deref(), Some("b"));
        assert_eq!(local_workspace(&state(vec![remote("r"), local("b")], "r")).as_deref(), Some("b"));
        assert_eq!(local_workspace(&state(vec![remote("r")], "r")), None);
    }
}
