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

/// A silent read in progress (Mac integration review 3): one stuck on a locked keychain never returns, so no other silent read starts
/// behind it (each would leak another blocked thread); they answer "the Keychain would ask" at once.
static SILENT_IN_FLIGHT: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// How long a read waits for the Keychain's UI lock (another read holding it may be stuck on a locked keychain).
const UI_WAIT: std::time::Duration = std::time::Duration::from_secs(5);

impl Silence {
    /// None when the Keychain is taken by a read that does not finish: a silent read already running, or the lock not free within
    /// [`UI_WAIT`].
    fn new(interactive: bool) -> Option<Silence> {
        if !interactive && SILENT_IN_FLIGHT.swap(true, std::sync::atomic::Ordering::SeqCst) {
            return None;
        }
        let Some(ui) = crate::secrets::keychain_ui_within(UI_WAIT) else {
            if !interactive {
                SILENT_IN_FLIGHT.store(false, std::sync::atomic::Ordering::SeqCst);
            }
            return None;
        };
        if interactive {
            return Some(Silence { previous: None, _ui: ui });
        }
        let mut previous: u8 = 1;
        // SAFETY: Security's process-wide switch, changed only while KEYCHAIN_UI is held.
        unsafe {
            SecKeychainGetUserInteractionAllowed(&mut previous);
            SecKeychainSetUserInteractionAllowed(0);
        }
        Some(Silence { previous: Some(previous), _ui: ui })
    }
}

impl Drop for Silence {
    fn drop(&mut self) {
        if let Some(previous) = self.previous {
            // SAFETY: as in `new` (the lock is released after this, with the guard).
            unsafe { SecKeychainSetUserInteractionAllowed(previous) };
            SILENT_IN_FLIGHT.store(false, std::sync::atomic::Ordering::SeqCst);
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
        let Some(_silence) = Silence::new(interactive) else {
            return Err(PlatformError::Refused("the Keychain is busy with a read that has not finished".into()));
        };
        let service = service(config_dir);
        // Swift adds `kSecUseAuthenticationUIFail` to every read, the interactive one too: an interactive read differs only in the
        // Keychain's user interaction being left on (the item's application-ACL dialog may show).
        let mut answer = Query::generic(&self.keychains, &service, self.user.as_deref()).data(true);
        if answer == Err(ERR_SEC_ITEM_NOT_FOUND) {
            answer = Query::generic(&self.keychains, &service, None).data(true);
        }
        answer_of(answer)
    }
}

/// A read's answer: the item's text (256 KB at most), a refusal where the Keychain would have asked, else no credential.
fn answer_of(answer: std::result::Result<Vec<u8>, i32>) -> Result<Option<String>> {
    match answer {
        Ok(bytes) if bytes.len() <= MAXIMUM_BYTES => Ok(String::from_utf8(bytes).ok()),
        Ok(_) => Ok(None),
        Err(ERR_SEC_INTERACTION_NOT_ALLOWED | ERR_SEC_AUTH_FAILED | ERR_SEC_USER_CANCELED) => {
            Err(PlatformError::Refused("the Keychain would ask for the Claude CLI's item".into()))
        }
        Err(_) => Ok(None),
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

    /// What a read answers for each status: the three a Keychain that would ask gives are a refusal (Swift's `keychainPermission`),
    /// anything else no credential. (A locked keychain cannot stand in for one that would ask: on the runner a silent read of a locked
    /// file keychain waits on its unlock and never returns, interaction off or not; the Swift app makes the same calls.)
    #[test]
    fn a_keychain_that_would_ask_is_a_refusal() {
        for status in [ERR_SEC_INTERACTION_NOT_ALLOWED, ERR_SEC_AUTH_FAILED, ERR_SEC_USER_CANCELED] {
            assert!(matches!(answer_of(Err(status)), Err(PlatformError::Refused(_))), "{status}");
        }
        assert_eq!(answer_of(Err(ERR_SEC_ITEM_NOT_FOUND)).unwrap(), None);
        assert_eq!(answer_of(Ok(b"{}".to_vec())).unwrap().as_deref(), Some("{}"));
        assert_eq!(answer_of(Ok(vec![b' '; MAXIMUM_BYTES + 1])).unwrap(), None);
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
