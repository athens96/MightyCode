//! M-B: remote workspaces' keys in the Keychain. Swift `MightyCore/Remote/RemoteKeychain.swift` (49 lines): a generic password of
//! service `dev.mightycode.native.remote` (the legacy `dev.mightyclaude.native.remote` copied forward on read), account = the
//! connection's id, `kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly`; contract `mighty_platform::secrets` (`SecretVault`).
//!
//! Tests use a throwaway file keychain under `$TMPDIR` (plan D21), never the login keychain.

use std::sync::Arc;

use mighty_platform::SecretVault;
use mighty_platform::unsupported::Unsupported;

/// The Mac's `SecretVault` (a missing secret, per the contract, until M-B).
pub fn secrets() -> Arc<dyn SecretVault> {
    Arc::new(Unsupported)
}
