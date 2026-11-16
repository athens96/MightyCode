//! M-B: the Claude CLI's own credential in the Keychain, for the account usage. Swift `AccountUsageService.swift:402-441`
//! (`ClaudeQuotaCredentials.read`); contract `mighty_platform::accounts` (`ClaudeCredentials`): the generic password of service
//! `Claude Code-credentials`, or `Claude Code-credentials-<the first 8 hex digits of SHA-256 of the config folder, NFC>` for another
//! config folder; account `NSUserName()`, then any; `kSecUseAuthenticationUIFail`, with the Keychain's user interaction off for the
//! whole of a read that may not ask (its application-ACL dialog is not covered by the flag). The CLI's `.credentials.json` and the
//! `usage.claudeKeychain` switch are the caller's (mighty-accounts `usage::claude_credential`).
//!
//! Tests use a throwaway file keychain (secrets.rs `Throwaway`), never the login keychain.

use std::path::Path;
use std::sync::Arc;

use mighty_platform::{ClaudeCredentials, PlatformError, Result};
use objc2_foundation::NSString;

use crate::secrets::{
    ERR_SEC_AUTH_FAILED, ERR_SEC_INTERACTION_NOT_ALLOWED, ERR_SEC_ITEM_NOT_FOUND, ERR_SEC_USER_CANCELED, Keychains, Query,
    SecKeychainGetUserInteractionAllowed, SecKeychainSetUserInteractionAllowed,
};

/// Swift's `parse` cap: a larger item is no credential.
const MAXIMUM_BYTES: usize = 262_144;

unsafe extern "C" {
    fn CC_SHA256(data: *const std::ffi::c_void, len: u32, md: *mut u8) -> *mut u8;
}

/// The Keychain service of a config folder: `Claude Code-credentials`, or with the first 8 hex digits of SHA-256 of the folder's
/// text in Unicode NFC (`precomposedStringWithCanonicalMapping`).
pub fn service(config_dir: Option<&Path>) -> String {
    match config_dir {
        None => "Claude Code-credentials".into(),
        Some(dir) => {
            let text = NSString::from_str(&dir.to_string_lossy()).precomposedStringWithCanonicalMapping().to_string();
            let mut digest = [0u8; 32];
            // SAFETY: the text's bytes and a 32-byte digest buffer.
            unsafe { CC_SHA256(text.as_ptr().cast(), text.len() as u32, digest.as_mut_ptr()) };
            let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
            format!("Claude Code-credentials-{}", &hex[..8])
        }
    }
}

/// `NSUserName()`: the login name of the user the process runs as.
fn user_name() -> Option<String> {
    let mut buffer = vec![0 as libc::c_char; 4096];
    let mut entry: libc::passwd = unsafe { std::mem::zeroed() };
    let mut found: *mut libc::passwd = std::ptr::null_mut();
    // SAFETY: getpwuid_r fills `entry` with pointers into `buffer`, both alive here.
    let status = unsafe { libc::getpwuid_r(libc::getuid(), &mut entry, buffer.as_mut_ptr(), buffer.len(), &mut found) };
    if status != 0 || found.is_null() || entry.pw_name.is_null() {
        return None;
    }
    // SAFETY: a NUL-terminated name in `buffer`.
    Some(unsafe { std::ffi::CStr::from_ptr(entry.pw_name) }.to_string_lossy().into_owned())
}

/// The Keychain's user interaction turned off for a read that may not ask, and put back as it was. It holds the process-wide
/// [`crate::secrets::KEYCHAIN_UI`] lock for its whole life (an interactive read holds it too), so no other Keychain read runs while the
/// switch is off and two silent reads never interleave their saves and restores.
struct Silence {
    previous: Option<u8>,
    _ui: std::sync::MutexGuard<'static, ()>,
}

impl Silence {
    fn new(interactive: bool) -> Silence {
        let ui = crate::secrets::keychain_ui();
        if interactive {
            return Silence { previous: None, _ui: ui };
        }
        let mut previous: u8 = 1;
        // SAFETY: Security's process-wide switch, changed only while KEYCHAIN_UI is held.
        unsafe {
            SecKeychainGetUserInteractionAllowed(&mut previous);
            SecKeychainSetUserInteractionAllowed(0);
        }
        Silence { previous: Some(previous), _ui: ui }
    }
}

impl Drop for Silence {
    fn drop(&mut self) {
        if let Some(previous) = self.previous {
            // SAFETY: as in `new` (the lock is released after this, with the guard).
            unsafe { SecKeychainSetUserInteractionAllowed(previous) };
        }
    }
}

/// The Claude CLI's Keychain item.
pub struct KeychainClaudeCredentials {
    keychains: Keychains,
    user: Option<String>,
}

impl KeychainClaudeCredentials {
    /// The user's default keychains, as this user.
    pub fn user() -> KeychainClaudeCredentials {
        KeychainClaudeCredentials { keychains: crate::secrets::Keychains::user_default(), user: user_name() }
    }

    #[cfg(test)]
    fn only(keychains: Keychains, user: &str) -> KeychainClaudeCredentials {
        KeychainClaudeCredentials { keychains, user: Some(user.to_string()) }
    }
}

impl ClaudeCredentials for KeychainClaudeCredentials {
    fn read(&self, config_dir: Option<&Path>, interactive: bool) -> Result<Option<String>> {
        if config_dir.is_some_and(|dir| !dir.is_absolute() || dir.to_string_lossy().contains('\0')) {
            return Ok(None);
        }
        let _silence = Silence::new(interactive);
        let service = service(config_dir);
        // Swift adds `kSecUseAuthenticationUIFail` to every read, the interactive one too: an interactive read differs only in the
        // Keychain's user interaction being left on (the item's application-ACL dialog may show).
        let mut answer = Query::generic(&self.keychains, &service, self.user.as_deref()).data(true);
        if answer == Err(ERR_SEC_ITEM_NOT_FOUND) {
            answer = Query::generic(&self.keychains, &service, None).data(true);
        }
        match answer {
            Ok(bytes) if bytes.len() <= MAXIMUM_BYTES => Ok(String::from_utf8(bytes).ok()),
            Ok(_) => Ok(None),
            Err(ERR_SEC_INTERACTION_NOT_ALLOWED | ERR_SEC_AUTH_FAILED | ERR_SEC_USER_CANCELED) => {
                Err(PlatformError::Refused("the Keychain would ask for the Claude CLI's item".into()))
            }
            Err(_) => Ok(None),
        }
    }
}

/// The Mac's `ClaudeCredentials`: the user's Keychain.
pub fn claude_credentials() -> Arc<dyn ClaudeCredentials> {
    Arc::new(KeychainClaudeCredentials::user())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::secrets::Throwaway;

    const ITEM: &str = r#"{"claudeAiOauth":{"accessToken":"sk-ant-throwaway","scopes":["user:profile"]}}"#;

    /// The default item by this user's account, else any account; another config folder's own item, never the default's.
    #[test]
    fn the_cli_item_is_read_by_account_then_any_and_per_config_folder() {
        let throwaway = Throwaway::new("claude-credentials");
        throwaway.add("Claude Code-credentials", "someone-else", ITEM.as_bytes());
        let store = KeychainClaudeCredentials::only(throwaway.keychains(), "me");
        assert_eq!(store.read(None, false).unwrap().as_deref(), Some(ITEM), "any account when this user's is missing");
        let work = Path::new("/Users/me/.claude-work");
        assert_eq!(store.read(Some(work), false).unwrap(), None, "a custom profile never borrows the default item");
        throwaway.add(&service(Some(work)), "me", b"{\"work\":true}");
        assert_eq!(store.read(Some(work), true).unwrap().as_deref(), Some("{\"work\":true}"));
        assert_eq!(store.read(Some(Path::new("relative")), false).unwrap(), None);
    }

    /// A Keychain that would ask (here: the throwaway keychain locked) refuses a silent read, without a prompt (the run would hang on
    /// one), which is Swift's `keychainPermission`.
    #[test]
    fn a_locked_keychain_refuses_a_silent_read() {
        unsafe extern "C" {
            fn SecKeychainLock(keychain: *const std::ffi::c_void) -> i32;
        }
        let throwaway = Throwaway::new("claude-locked");
        throwaway.add("Claude Code-credentials", "me", ITEM.as_bytes());
        // SAFETY: the test's own keychain.
        assert_eq!(unsafe { SecKeychainLock(core_foundation::base::TCFType::as_concrete_TypeRef(&throwaway.keychain) as *const _) }, 0);
        let store = KeychainClaudeCredentials::only(throwaway.keychains(), "me");
        assert!(matches!(store.read(None, false), Err(PlatformError::Refused(_))));
    }

    /// The service names the CLI uses: the plain one, and SHA-256 of the folder's NFC text.
    #[test]
    fn the_service_names_follow_the_cli() {
        assert_eq!(service(None), "Claude Code-credentials");
        let named = service(Some(Path::new("/tmp/x")));
        assert!(named.starts_with("Claude Code-credentials-") && named.len() == "Claude Code-credentials-".len() + 8);
        // A decomposed "é" names the same item as the composed one.
        assert_eq!(service(Some(Path::new("/tmp/e\u{301}"))), service(Some(Path::new("/tmp/\u{e9}"))));
    }
}
