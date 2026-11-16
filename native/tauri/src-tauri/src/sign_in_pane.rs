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
    let plain = !word.is_empty() && word.chars().all(|c| c.is_ascii_alphanumeric() || "_./:@%+=,-".contains(c));
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

impl TerminalLauncher for PaneSignIn {
    /// A local workspace is made the active one (the active workspace when it is local, else the first local one); the pane is added
    /// and the command typed into it once its shell is up (styles.rs `run_terminal_command`, C#'s and Swift's `autoRun`).
    fn open(&self, launch: TerminalLaunch) -> mighty_platform::Result<Box<dyn TerminalWindow>> {
        let Some(host) = self.app.try_state::<Arc<Host>>().map(|host| host.inner().clone()) else {
            return Err(PlatformError::unsupported("terminal pane", "open"));
        };
        let command = command_line(&launch).ok_or_else(|| PlatformError::Invalid("no command".into()))?;
        {
            let mut view = host.view();
            let local = |id: &str| view.snapshot.workspace(id).is_some_and(|w| w.remote.is_none());
            let active = view.snapshot.active_workspace_id.clone().filter(|id| local(id));
            let chosen = active.or_else(|| view.snapshot.workspaces().iter().find(|w| w.remote.is_none()).map(|w| w.id().to_string()));
            let Some(workspace) = chosen else {
                return Err(PlatformError::Refused(host.locale.get("settings.cliAccounts.noWorkspace")));
            };
            if view.snapshot.active_workspace_id.as_deref() != Some(workspace.as_str()) {
                crate::frame::select_workspace(&mut view, &workspace).map_err(PlatformError::Refused)?;
            }
        }
        host.schedule();
        let title = title(&launch, &host.locale);
        tauri::async_runtime::spawn(async move {
            if let Err(error) = crate::styles::run_terminal_command(&host, &command, &title, true).await {
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
        assert_eq!(command_line(&launch(&[], &[])), None);
        let l = mighty_core::locale::Locale::new("en");
        assert_eq!(title(&launch(&["/opt/bin/agy"], &[]), &l), "Antigravity sign-in");
        assert_eq!(title(&launch(&["claude", "/setup-bedrock"], &[]), &l), "Claude Bedrock setup");
    }
}
