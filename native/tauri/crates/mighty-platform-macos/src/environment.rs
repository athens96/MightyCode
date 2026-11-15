//! M-A: the login shell's environment for the CLIs. Swift `MightyCore/CLIEnvironmentResolver.swift` (122 lines); contract
//! `mighty_platform::environment` (`ShellEnvironment`: never fails, 4 s, reused 10 s).
//!
//! To port (plan §1.3): `/bin/zsh -ilc "printf '\0MIGHTY_ENV_<uuid>\0'; /usr/bin/env -0; printf '\0MIGHTY_END_<uuid>\0'"`, a 4 s
//! timeout, 1 MB cap, cwd the workspace or home; a 50 ms coalesce, 10 s reuse per cwd, 64 entries; duplicate keys and keys holding
//! `=` refused, a non-empty PATH required; PATH gets `~/.local/bin`, `~/.npm-global/bin`, `~/.bun/bin`, `/opt/homebrew/bin`,
//! `/usr/local/bin`; a failure falls back to the process environment without echoing the shell's output.

use std::sync::Arc;

use mighty_platform::ShellEnvironment;
use mighty_platform::unsupported::Unsupported;

/// The Mac's `ShellEnvironment` (the process's own environment until M-A: `Unsupported`'s contract-keeping fallback).
pub fn shell_environment() -> Arc<dyn ShellEnvironment> {
    Arc::new(Unsupported)
}
