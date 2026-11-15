//! The Claude CLI's own stored credential, which the account usage reads to ask Anthropic for the plan's limits (Swift
//! `AccountUsageService.swift:401-451`). Mac stage Step 0: the port and its fake; the Mac reader is M-B's (`mighty-platform-macos`
//! `claude_credentials`), its reader in `mighty-accounts`' usage too. Windows reads the CLI's `.credentials.json` itself and keeps
//! `Unsupported` here.
//!
//! On the Mac the CLI keeps it in the login Keychain: a generic password of service `Claude Code-credentials`, or
//! `Claude Code-credentials-<the first 8 hex digits of SHA-256(CLAUDE_CONFIG_DIR)>` for another config folder; account
//! `NSUserName()`, else any. A read that may not show UI (`kSecUseAuthenticationUIFail`, interaction off) answers None where the
//! Keychain would have to ask; the CLI's fallback file `<config>/.credentials.json` (256 KB at most) is read after it. Swift reads the
//! Keychain only while the defaults key `usage.claudeKeychain` allows it.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use crate::Result;

/// The Claude CLI's stored OAuth credential (its JSON, as the CLI wrote it).
pub trait ClaudeCredentials: Send + Sync {
    /// The credential for `config_dir` (`CLAUDE_CONFIG_DIR`; None: the default `~/.claude`), or None when there is none or it cannot
    /// be read without asking (`interactive` false). The text is a secret: it never reaches a log, an error or `Debug`.
    fn read(&self, config_dir: Option<&Path>, interactive: bool) -> Result<Option<String>>;
}

/// A test's stored credential: answers `stored` (to an interactive read only when `needs_interaction`) and records every read.
#[derive(Default)]
pub struct FakeClaudeCredentials {
    stored: Mutex<Option<String>>,
    needs_interaction: Mutex<bool>,
    reads: Mutex<Vec<(Option<PathBuf>, bool)>>,
}

impl FakeClaudeCredentials {
    pub fn with(stored: &str, needs_interaction: bool) -> FakeClaudeCredentials {
        FakeClaudeCredentials {
            stored: Mutex::new(Some(stored.to_string())),
            needs_interaction: Mutex::new(needs_interaction),
            reads: Mutex::default(),
        }
    }

    /// Every read: its config folder and whether it was interactive.
    pub fn reads(&self) -> Vec<(Option<PathBuf>, bool)> {
        self.reads.lock().unwrap_or_else(|p| p.into_inner()).clone()
    }
}

impl std::fmt::Debug for FakeClaudeCredentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FakeClaudeCredentials").field("reads", &self.reads()).finish_non_exhaustive()
    }
}

impl ClaudeCredentials for FakeClaudeCredentials {
    fn read(&self, config_dir: Option<&Path>, interactive: bool) -> Result<Option<String>> {
        self.reads.lock().unwrap_or_else(|p| p.into_inner()).push((config_dir.map(Path::to_path_buf), interactive));
        let needs_interaction = *self.needs_interaction.lock().unwrap_or_else(|p| p.into_inner());
        if needs_interaction && !interactive {
            return Ok(None);
        }
        Ok(self.stored.lock().unwrap_or_else(|p| p.into_inner()).clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The contract's one rule the shared code leans on: a read that may not ask gets nothing where the store would have to ask, and
    /// the secret never shows in `Debug`.
    #[test]
    fn a_silent_read_gets_nothing_where_the_store_would_ask() {
        let store = FakeClaudeCredentials::with("{\"claudeAiOauth\":{\"accessToken\":\"sk-secret\"}}", true);
        let port: &dyn ClaudeCredentials = &store;
        assert_eq!(port.read(None, false).unwrap(), None);
        assert!(port.read(Some(Path::new("/c")), true).unwrap().is_some_and(|text| text.contains("accessToken")));
        assert_eq!(store.reads(), vec![(None, false), (Some(PathBuf::from("/c")), true)]);
        assert!(!format!("{store:?}").contains("sk-secret"));
    }
}
