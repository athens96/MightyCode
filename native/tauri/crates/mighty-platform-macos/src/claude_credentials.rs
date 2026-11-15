//! M-B: the Claude CLI's own credential for the account usage. Swift `AccountUsageService.swift:401-451`; contract
//! `mighty_platform::accounts` (`ClaudeCredentials`: the Keychain service `Claude Code-credentials[-<8 hex of SHA-256 of the
//! config folder>]`, account `NSUserName()` then any, no UI on a silent read, the `.credentials.json` fallback of 256 KB at most,
//! gated by the defaults key `usage.claudeKeychain`). Tests use a throwaway file keychain.

use std::sync::Arc;

use mighty_platform::ClaudeCredentials;
use mighty_platform::unsupported::Unsupported;

/// The Mac's `ClaudeCredentials` (Unsupported until M-B).
pub fn claude_credentials() -> Arc<dyn ClaudeCredentials> {
    Arc::new(Unsupported)
}
