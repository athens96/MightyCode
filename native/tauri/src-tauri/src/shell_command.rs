//! M-F: the Workspace menu's "Install shell command 'mighty'": `~/.local/bin/mighty`, mode 0755, the script of Swift
//! `OSIntegration.swift:38-76` (`ShellCommand`), then whether that folder is on the login shell's PATH (the CLI environment the runs
//! use). Only on the menu item, where the write policy lets the app change the machine (plan §6), never in a smoke; tests use a
//! temporary HOME (Swift `OSIntegrationTests.swift:18` "theShellCommandInstallsExecutable").

use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::json;

use crate::host::Host;

/// The command's name.
pub const NAME: &str = "mighty";

/// Swift `ShellCommand.script`, byte for byte (its file ends with one more newline).
pub const SCRIPT: &str = r#"#!/bin/sh
# Mighty Code: open a folder (default: the current one) as a workspace,
# with an optional request put in the composer (never sent).
#   mighty                 open this folder
#   mighty ../other        open another folder
#   mighty "fix the tests" open this folder with a request
dir="$PWD"
if [ $# -gt 0 ] && [ -d "$1" ]; then dir="$(cd "$1" && pwd)"; shift; fi
enc() { osascript -l JavaScript -e 'function run(a) { return encodeURIComponent(a[0]) }' "$1"; }
url="mightycode://open?path=$(enc "$dir")"
if [ $# -gt 0 ]; then url="$url&prompt=$(enc "$*")"; fi
exec open "$url""#;

/// `~/.local/bin`, the folder the command goes in (no administrator needed).
pub fn folder(home: &Path) -> PathBuf {
    home.join(".local").join("bin")
}

/// Writes the command under `home` (atomically, then executable); its path.
pub fn install(home: &Path) -> Result<PathBuf, String> {
    let folder = folder(home);
    std::fs::create_dir_all(&folder).map_err(|e| e.to_string())?;
    let path = folder.join(NAME);
    let staged = folder.join(format!(".{NAME}.{}.tmp", std::process::id()));
    std::fs::write(&staged, format!("{SCRIPT}\n")).map_err(|e| e.to_string())?;
    executable(&staged).map_err(|e| e.to_string())?;
    std::fs::rename(&staged, &path).map_err(|e| {
        let _ = std::fs::remove_file(&staged);
        e.to_string()
    })?;
    Ok(path)
}

#[cfg(unix)]
fn executable(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt as _;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))
}

#[cfg(not(unix))]
fn executable(_path: &Path) -> std::io::Result<()> {
    Ok(())
}

/// Swift `ShellCommand.onPath`: whether `folder` is one of `path`'s entries, compared standardized.
pub fn on_path(path: Option<&str>, folder: &Path) -> bool {
    let wanted = crate::deep_link::standardized(&folder.to_string_lossy());
    path.unwrap_or("").split(':').any(|entry| entry.starts_with('/') && crate::deep_link::standardized(entry) == wanted)
}

/// The menu item (Swift `installShellCommand`): the command written, and the page told where and whether the shell finds it.
pub async fn install_from_menu(host: &Arc<Host>) {
    let l = &host.locale;
    if !host.may_change_machine() {
        host.log.line("shell command: not installed (this run may not change the machine: --allow-state-write)");
        host.view().error = Some(l.get("windows.notifications.statusVerificationMode"));
        return;
    }
    let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else { return };
    let path = match install(&home) {
        Ok(path) => path,
        Err(error) => {
            host.view().error = Some(error);
            return;
        }
    };
    // The login shell's PATH, as the runs see it (a GUI app's own PATH is launchd's short one).
    let shell_path = match host.runs() {
        Some(runs) => runs.catalog.environment().resolve(None, false).await.values.get("PATH").cloned(),
        None => None,
    };
    let (path_text, folder_text) = (path.display().to_string(), folder(&home).display().to_string());
    let message = if on_path(shell_path.as_deref(), &folder(&home)) {
        l.get_with("shellCommand.installed.ready", &[("path", &path_text)])
    } else {
        l.get_with("shellCommand.installed.addPath", &[("path", &path_text), ("folder", &folder_text)])
    };
    let Some(_sheet) = crate::frame::Sheet::begin(host) else { return };
    let (_, answer) = crate::dialogs::of(host).open(json!({
        "type": "confirm", "title": l.get("shellCommand.installed.title"), "message": message, "close": l.get("common.close"),
    }));
    let _ = answer.await;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Swift `OSIntegrationTests.swift:18` "theShellCommandInstallsExecutable": under a temporary HOME only.
    #[test]
    fn the_shell_command_installs_executable() {
        let home = std::env::temp_dir().join(format!("mighty-home-{}-{}", std::process::id(), mighty_core::wire::new_id()));
        let path = install(&home).unwrap();
        assert!(path.ends_with(".local/bin/mighty"));
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.starts_with("#!/bin/sh") && text.ends_with("exec open \"$url\"\n"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o755);
        }
        let bin = format!("/usr/bin:{}/.local/bin/", home.display());
        assert!(on_path(Some(&bin), &folder(&home)));
        assert!(!on_path(Some("/usr/bin"), &folder(&home)));
        // Installed again: replaced in place, nothing left beside it.
        install(&home).unwrap();
        assert_eq!(std::fs::read_dir(folder(&home)).unwrap().count(), 1);
        std::fs::remove_dir_all(&home).unwrap();
    }
}
