//! M-F: the Workspace menu's "Install shell command 'mighty'": `~/.local/bin/mighty`, mode 0755, the script of Swift
//! `OSIntegration.swift:38-76`, then whether that folder is on the login shell's PATH (the platform's `ShellEnvironment`). Only on the
//! menu item, never in a smoke; tests use a temporary HOME (Swift `OSIntegrationTests.swift:18` "theShellCommandInstallsExecutable").

use std::path::{Path, PathBuf};

/// Writes the command under `home`; its path. Step 0: not written yet (M-F).
#[allow(dead_code, reason = "M-F: the menu's item installs the command")]
pub fn install(_home: &Path) -> Result<PathBuf, String> {
    Err("installing the shell command is the Mac stage's (M-F)".into())
}
