//! M-B: remote workspaces' keys in the Keychain. Swift `MightyCore/Remote/RemoteKeychain.swift` (49 lines): a generic password of
//! service `dev.mightycode.native.remote` (a key found only under the legacy `dev.mightyclaude.native.remote` copied forward when
//! first read), account = the connection's id, `kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly`; contract `mighty_platform::secrets`
//! (`SecretVault`). The Keychain queries here are shared with `claude_credentials.rs`.
//!
//! The phone pairing's secrets (`mighty_platform::pairing::vaulted`) are kept the same way under their own service,
//! `dev.mightycode.native.pairing` (account = the item the pairing files name), with the same attributes, so the app that made the
//! item reads it at every launch without asking ([`KeychainVault::pairing`]).
//!
//! The app reads the user's default keychains; tests give a throwaway file keychain under `$TMPDIR` (`SecKeychainCreate`, deleted at
//! the end), never the login keychain.

use std::sync::Arc;

use core_foundation::array::CFArray;
use core_foundation::base::{CFType, CFTypeRef, TCFType};
use core_foundation::boolean::CFBoolean;
use core_foundation::data::CFData;
use core_foundation::dictionary::{CFDictionary, CFDictionaryRef};
use core_foundation::string::{CFString, CFStringRef};
use mighty_platform::{PlatformError, Result, SecretVault};

pub(crate) const ERR_SEC_SUCCESS: i32 = 0;
pub(crate) const ERR_SEC_ITEM_NOT_FOUND: i32 = -25300;
pub(crate) const ERR_SEC_INTERACTION_NOT_ALLOWED: i32 = -25308;
pub(crate) const ERR_SEC_AUTH_FAILED: i32 = -25293;
pub(crate) const ERR_SEC_USER_CANCELED: i32 = -128;

#[link(name = "Security", kind = "framework")]
unsafe extern "C" {
    static kSecClass: CFStringRef;
    static kSecClassGenericPassword: CFStringRef;
    static kSecAttrService: CFStringRef;
    static kSecAttrAccount: CFStringRef;
    static kSecAttrAccessible: CFStringRef;
    static kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly: CFStringRef;
    static kSecReturnData: CFStringRef;
    static kSecMatchLimit: CFStringRef;
    static kSecMatchLimitOne: CFStringRef;
    static kSecMatchSearchList: CFStringRef;
    static kSecUseKeychain: CFStringRef;
    static kSecValueData: CFStringRef;
    static kSecUseAuthenticationUI: CFStringRef;
    static kSecUseAuthenticationUIFail: CFStringRef;
    fn SecItemCopyMatching(query: CFDictionaryRef, result: *mut CFTypeRef) -> i32;
    fn SecItemAdd(attributes: CFDictionaryRef, result: *mut CFTypeRef) -> i32;
    fn SecItemUpdate(query: CFDictionaryRef, update: CFDictionaryRef) -> i32;
    fn SecItemDelete(query: CFDictionaryRef) -> i32;
    pub(crate) fn SecKeychainGetUserInteractionAllowed(state: *mut u8) -> i32;
    pub(crate) fn SecKeychainSetUserInteractionAllowed(state: u8) -> i32;
}

/// `SecKeychainSetUserInteractionAllowed` is process-wide: a silent Claude read turns it off for its whole read
/// (claude_credentials.rs `Silence`), so every Keychain read that might ask holds this lock, and two silent reads never overlap (the
/// second would put back the first's "off" for good).
pub(crate) static KEYCHAIN_UI: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// [`KEYCHAIN_UI`] within `wait`, else None (Mac integration review 3): a silent read stuck on a locked keychain holds it with no
/// limit (the Security framework's call does not return until the unlock), and nothing else may wait on it for ever.
pub(crate) fn keychain_ui_within(wait: std::time::Duration) -> Option<std::sync::MutexGuard<'static, ()>> {
    let until = std::time::Instant::now() + wait;
    loop {
        match KEYCHAIN_UI.try_lock() {
            Ok(guard) => return Some(guard),
            Err(std::sync::TryLockError::Poisoned(poisoned)) => return Some(poisoned.into_inner()),
            Err(std::sync::TryLockError::WouldBlock) if std::time::Instant::now() < until => {
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            Err(std::sync::TryLockError::WouldBlock) => return None,
        }
    }
}

/// How long a vault read waits for [`KEYCHAIN_UI`] before it reads as no key.
const VAULT_WAIT: std::time::Duration = std::time::Duration::from_secs(2);

/// A Security constant as a CFString.
fn key(name: CFStringRef) -> CFString {
    // SAFETY: the framework's constant strings live for the process.
    unsafe { CFString::wrap_under_get_rule(name) }
}

/// Which keychains the queries see: the user's default ones, or one keychain (a test's throwaway file keychain).
#[derive(Clone)]
pub struct Keychains {
    only: Option<security_framework::os::macos::keychain::SecKeychain>,
}

impl Keychains {
    /// The user's default keychains.
    pub(crate) fn user_default() -> Keychains {
        Keychains { only: None }
    }
}

// SAFETY: a SecKeychainRef is a CF object; Security's item calls are thread-safe.
unsafe impl Send for Keychains {}
unsafe impl Sync for Keychains {}

/// One generic-password query: the class, service and account (when given), and the keychain it is confined to.
pub(crate) struct Query {
    pairs: Vec<(CFString, CFType)>,
}

impl Query {
    pub(crate) fn generic(keychains: &Keychains, service: &str, account: Option<&str>) -> Query {
        let mut pairs = vec![
            (key(unsafe { kSecClass }), key(unsafe { kSecClassGenericPassword }).as_CFType()),
            (key(unsafe { kSecAttrService }), CFString::new(service).as_CFType()),
        ];
        if let Some(account) = account {
            pairs.push((key(unsafe { kSecAttrAccount }), CFString::new(account).as_CFType()));
        }
        if let Some(keychain) = &keychains.only {
            pairs.push((key(unsafe { kSecMatchSearchList }), CFArray::from_CFTypes(std::slice::from_ref(keychain)).as_CFType()));
        }
        Query { pairs }
    }

    fn with(mut self, name: CFStringRef, value: CFType) -> Query {
        self.pairs.push((key(name), value));
        self
    }

    fn dictionary(&self) -> CFDictionary<CFString, CFType> {
        CFDictionary::from_CFType_pairs(&self.pairs)
    }

    /// `SecItemCopyMatching` for the item's data (`kSecMatchLimitOne`); `ui_fail` adds `kSecUseAuthenticationUIFail` (the Claude
    /// item's reads; Swift's `RemoteKeychain` adds nothing).
    pub(crate) fn data(self, ui_fail: bool) -> std::result::Result<Vec<u8>, i32> {
        let mut query = self
            .with(unsafe { kSecReturnData }, CFBoolean::true_value().as_CFType())
            .with(unsafe { kSecMatchLimit }, key(unsafe { kSecMatchLimitOne }).as_CFType());
        if ui_fail {
            query = query.with(unsafe { kSecUseAuthenticationUI }, key(unsafe { kSecUseAuthenticationUIFail }).as_CFType());
        }
        let dictionary = query.dictionary();
        let mut result: CFTypeRef = std::ptr::null();
        // SAFETY: a valid query dictionary; `result` receives a +1 reference on success.
        let status = unsafe { SecItemCopyMatching(dictionary.as_concrete_TypeRef(), &mut result) };
        if status != ERR_SEC_SUCCESS {
            return Err(status);
        }
        if result.is_null() {
            return Err(ERR_SEC_ITEM_NOT_FOUND);
        }
        // SAFETY: the copy's result, owned here.
        let value = unsafe { CFType::wrap_under_create_rule(result) };
        value.downcast::<CFData>().map(|data| data.bytes().to_vec()).ok_or(ERR_SEC_ITEM_NOT_FOUND)
    }
}

/// The remote keys' Keychain (or the phone pairing's: [`KeychainVault::pairing`]).
pub struct KeychainVault {
    keychains: Keychains,
    service: &'static str,
    /// Where items of this service were kept before the rename (copied forward when read).
    legacy: Option<&'static str>,
    /// Whether an item's text is one this service keeps (any other reads as none).
    valid: fn(&str) -> bool,
}

/// Swift `RemoteKeychain.service`.
pub const SERVICE: &str = "dev.mightycode.native.remote";
/// Swift `RemoteKeychain.legacyService`: where the app kept the keys before it was renamed to Mighty Code.
pub const LEGACY_SERVICE: &str = "dev.mightyclaude.native.remote";
/// The phone pairing's secrets (new in the Rust app; no legacy service).
pub const PAIRING_SERVICE: &str = "dev.mightycode.native.pairing";

/// `RemoteWorkspaceValidation.key`: `^[A-Za-z0-9_-]{43,128}$`, the same rule as the pairing key's; or a computer paired through the
/// relay's secret (mighty-remote `PairedSecret::encode`: `{"v":1,"relay","sid","pk","clientId",…}`, which that crate checks member by
/// member when it reads it back), kept in the same service under its connection's account.
fn remote_key(text: &str) -> bool {
    mighty_platform::pairing::four_files::pairing_key(text) || paired_secret(text)
}

/// A paired connection's secret by its shape: a small JSON object of version 1 naming its relay server.
fn paired_secret(text: &str) -> bool {
    text.len() <= 4096
        && serde_json::from_str::<serde_json::Value>(text).ok().is_some_and(|value| {
            value.get("v").and_then(serde_json::Value::as_i64) == Some(1) && value.get("sid").is_some_and(serde_json::Value::is_string)
        })
}

/// The pairing vault's text (`mighty_platform::pairing::vaulted::Secrets`).
fn pairing_secrets(text: &str) -> bool {
    mighty_platform::pairing::vaulted::Secrets::decode(text).is_some()
}

impl KeychainVault {
    /// The user's default keychains (the login keychain).
    pub fn user() -> KeychainVault {
        KeychainVault { keychains: Keychains { only: None }, service: SERVICE, legacy: Some(LEGACY_SERVICE), valid: remote_key }
    }

    /// One keychain only (a test's throwaway file keychain).
    pub fn only(keychain: security_framework::os::macos::keychain::SecKeychain) -> KeychainVault {
        KeychainVault { keychains: Keychains { only: Some(keychain) }, ..KeychainVault::user() }
    }

    /// The phone pairing's secrets in the user's default keychains, service [`PAIRING_SERVICE`].
    pub fn pairing() -> KeychainVault {
        KeychainVault { keychains: Keychains { only: None }, service: PAIRING_SERVICE, legacy: None, valid: pairing_secrets }
    }

    /// [`KeychainVault::pairing`] in one keychain only (a test's throwaway file keychain).
    pub fn pairing_only(keychain: security_framework::os::macos::keychain::SecKeychain) -> KeychainVault {
        KeychainVault { keychains: Keychains { only: Some(keychain) }, ..KeychainVault::pairing() }
    }

    /// The item's text when this service keeps text of its shape (`valid`), read while no silent Claude read has the Keychain's
    /// user interaction off ([`KEYCHAIN_UI`]).
    fn read(&self, id: &str, service: &str) -> Option<String> {
        let bytes = {
            // A Claude read stuck on a locked keychain keeps the lock: past 2 s the secret reads as missing (a remote connection
            // fails and says so; the phone remote does not start this launch) rather than hanging the caller.
            let Some(_ui) = keychain_ui_within(VAULT_WAIT) else {
                eprintln!("mighty: the Keychain is busy with another read; a secret's read gave up after 2 s");
                return None;
            };
            Query::generic(&self.keychains, service, Some(id)).data(false).ok()?
        };
        String::from_utf8(bytes).ok().filter(|text| (self.valid)(text))
    }

    /// `SecItemUpdate`, else `SecItemAdd` with `kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly` (into the confined keychain).
    fn write(&self, id: &str, secret: &str) -> std::result::Result<(), i32> {
        let query = Query::generic(&self.keychains, self.service, Some(id));
        let value = CFData::from_buffer(secret.as_bytes());
        let update = CFDictionary::from_CFType_pairs(&[(key(unsafe { kSecValueData }), value.as_CFType())]);
        // SAFETY: valid dictionaries.
        let status = unsafe { SecItemUpdate(query.dictionary().as_concrete_TypeRef(), update.as_concrete_TypeRef()) };
        if status == ERR_SEC_SUCCESS {
            return Ok(());
        }
        if status != ERR_SEC_ITEM_NOT_FOUND {
            return Err(status);
        }
        let mut item = Query::generic(&Keychains { only: None }, self.service, Some(id))
            .with(unsafe { kSecValueData }, value.as_CFType())
            .with(unsafe { kSecAttrAccessible }, key(unsafe { kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly }).as_CFType());
        if let Some(keychain) = &self.keychains.only {
            item = item.with(unsafe { kSecUseKeychain }, keychain.as_CFType());
        }
        // SAFETY: a valid attribute dictionary; no result asked for.
        let status = unsafe { SecItemAdd(item.dictionary().as_concrete_TypeRef(), std::ptr::null_mut()) };
        if status == ERR_SEC_SUCCESS { Ok(()) } else { Err(status) }
    }
}

impl SecretVault for KeychainVault {
    /// `RemoteKeychain.load`: the item under the service, else the legacy service's, copied forward (a failed copy still answers).
    fn load(&self, id: &str) -> Option<String> {
        if let Some(secret) = self.read(id, self.service) {
            return Some(secret);
        }
        let secret = self.read(id, self.legacy?)?;
        let _ = self.write(id, &secret);
        Some(secret)
    }

    fn save(&self, id: &str, secret: &str) -> Result<()> {
        // The status names the failure, never the secret or the id.
        self.write(id, secret).map_err(|status| PlatformError::Refused(format!("the Keychain refused the key (OSStatus {status})")))
    }

    fn delete(&self, id: &str) -> Result<()> {
        let query = Query::generic(&self.keychains, self.service, Some(id));
        // SAFETY: a valid query dictionary.
        let status = unsafe { SecItemDelete(query.dictionary().as_concrete_TypeRef()) };
        match status {
            ERR_SEC_SUCCESS | ERR_SEC_ITEM_NOT_FOUND => Ok(()),
            status => Err(PlatformError::Refused(format!("the Keychain refused the delete (OSStatus {status})"))),
        }
    }
}

/// The Mac's `SecretVault`: the user's Keychain.
pub fn secrets() -> Arc<dyn SecretVault> {
    Arc::new(KeychainVault::user())
}

/// A throwaway file keychain under `$TMPDIR` for tests, deleted (from disk and the search list) when dropped.
#[cfg(test)]
pub(crate) struct Throwaway {
    pub keychain: security_framework::os::macos::keychain::SecKeychain,
    path: std::path::PathBuf,
}

#[cfg(test)]
impl Throwaway {
    pub fn new(name: &str) -> Throwaway {
        let path = std::env::temp_dir().join(format!("mighty-{name}-{}-{}.keychain-db", std::process::id(), line!()));
        let _ = std::fs::remove_file(&path);
        let keychain = security_framework::os::macos::keychain::CreateOptions::new()
            .password("throwaway")
            .create(&path)
            .expect("a throwaway keychain");
        Throwaway { keychain, path }
    }

    /// An item added straight into the throwaway keychain (what another app, or the Swift app under its old id, left there).
    pub fn add(&self, service: &str, account: &str, data: &[u8]) {
        let item = Query::generic(&Keychains { only: None }, service, Some(account))
            .with(unsafe { kSecValueData }, CFData::from_buffer(data).as_CFType())
            .with(unsafe { kSecUseKeychain }, self.keychain.as_CFType());
        // SAFETY: a valid attribute dictionary.
        let status = unsafe { SecItemAdd(item.dictionary().as_concrete_TypeRef(), std::ptr::null_mut()) };
        assert_eq!(status, ERR_SEC_SUCCESS, "adding a test item");
    }

    pub fn keychains(&self) -> Keychains {
        Keychains { only: Some(self.keychain.clone()) }
    }
}

#[cfg(test)]
impl Drop for Throwaway {
    fn drop(&mut self) {
        unsafe extern "C" {
            fn SecKeychainDelete(keychain: *const std::ffi::c_void) -> i32;
        }
        // SAFETY: the keychain this test made.
        unsafe { SecKeychainDelete(self.keychain.as_concrete_TypeRef() as *const std::ffi::c_void) };
        let _ = std::fs::remove_file(&self.path);
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_paired_computers_secret_passes_the_remote_services_check() {
        let secret = r#"{"v":1,"relay":"wss://relay.example","sid":"abc","pk":"x","clientId":"c","deviceToken":"t"}"#;
        assert!(super::remote_key(secret));
        assert!(!super::remote_key(r#"{"v":2,"sid":"abc"}"#));
        assert!(!super::remote_key("not a key"));
    }

    use super::*;

    /// Mac integration review 3: while another read holds the Keychain's lock (one stuck on a locked keychain), a wait for it gives up
    /// at its bound instead of blocking for ever; once it is let go, the lock is taken.
    #[test]
    fn a_wait_for_the_keychain_lock_gives_up_at_its_bound() {
        let (held, release) = (std::sync::mpsc::channel::<()>(), std::sync::mpsc::channel::<()>());
        let holder = std::thread::spawn(move || {
            let _ui = keychain_ui_within(std::time::Duration::from_secs(5)).expect("free");
            held.0.send(()).unwrap();
            release.1.recv().unwrap();
        });
        held.1.recv().unwrap();
        let started = std::time::Instant::now();
        assert!(keychain_ui_within(std::time::Duration::from_millis(200)).is_none(), "taken by the stuck read");
        assert!(started.elapsed() < std::time::Duration::from_secs(2), "it gave up at its bound");
        release.0.send(()).unwrap();
        holder.join().unwrap();
        assert!(keychain_ui_within(std::time::Duration::from_secs(1)).is_some(), "free again");
    }

    const FIRST: &str = "AQgPFh0kKzI5QEdOVVxjanF4f4aNlJuiqbC3vsXM09o";
    const SECOND: &str = "BwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwc";

    /// New, with no Swift test (none exists for `RemoteKeychain`): a key saved, replaced, read back and deleted, in a throwaway file
    /// keychain; a delete of a missing key succeeds; an item that has no key's shape reads as none (`RemoteWorkspaceValidation.key`).
    #[test]
    fn remote_keys_round_trip_in_a_throwaway_keychain() {
        let throwaway = Throwaway::new("remote-keys");
        let vault = KeychainVault::only(throwaway.keychain.clone());
        assert_eq!(vault.load("conn-1"), None);
        vault.save("conn-1", FIRST).unwrap();
        assert_eq!(vault.load("conn-1").as_deref(), Some(FIRST));
        vault.save("conn-1", SECOND).unwrap();
        assert_eq!(vault.load("conn-1").as_deref(), Some(SECOND));
        vault.delete("conn-1").unwrap();
        vault.delete("conn-1").unwrap();
        assert_eq!(vault.load("conn-1"), None);
        throwaway.add(SERVICE, "conn-bad", b"not a key");
        assert_eq!(vault.load("conn-bad"), None);
    }

    /// New: the phone pairing's secrets in their own service of a throwaway keychain: saved, read back, replaced, deleted; text that
    /// is not the pairing vault's reads as none; neither service sees the other's items.
    #[test]
    fn pairing_secrets_round_trip_in_their_own_service() {
        use mighty_platform::pairing::vaulted::Secrets;
        let throwaway = Throwaway::new("pairing-secrets");
        let vault = KeychainVault::pairing_only(throwaway.keychain.clone());
        let secrets = Secrets { pairing_key: FIRST.into(), host_token: "ab".repeat(32), secret_key: [5; 32] };
        let account = mighty_platform::pairing::vaulted::new_account();
        assert_eq!(vault.load(&account), None);
        vault.save(&account, &secrets.encode()).unwrap();
        assert_eq!(vault.load(&account).and_then(|t| Secrets::decode(&t)), Some(secrets.clone()));
        assert_eq!(KeychainVault::only(throwaway.keychain.clone()).load(&account), None, "not a remote key");
        let raw = Query::generic(&throwaway.keychains(), PAIRING_SERVICE, Some(&account)).data(false).unwrap();
        assert_eq!(raw, secrets.encode().into_bytes());
        let rotated = Secrets { pairing_key: SECOND.into(), ..secrets };
        vault.save(&account, &rotated.encode()).unwrap();
        assert_eq!(vault.load(&account).and_then(|t| Secrets::decode(&t)), Some(rotated));
        vault.delete(&account).unwrap();
        assert_eq!(vault.load(&account), None);
        throwaway.add(PAIRING_SERVICE, "junk", FIRST.as_bytes());
        assert_eq!(vault.load("junk"), None, "a remote key's shape is not the pairing vault's");
    }

    /// New, with no Swift test: a key found only under the pre-rename service is answered and copied to the current one; one without a
    /// key's shape is neither answered nor copied.
    #[test]
    fn a_legacy_service_item_is_copied_forward_on_read() {
        let throwaway = Throwaway::new("remote-legacy");
        throwaway.add(LEGACY_SERVICE, "conn-old", FIRST.as_bytes());
        throwaway.add(LEGACY_SERVICE, "conn-junk", b"junk");
        let vault = KeychainVault::only(throwaway.keychain.clone());
        assert_eq!(vault.load("conn-old").as_deref(), Some(FIRST));
        assert_eq!(vault.read("conn-old", SERVICE).as_deref(), Some(FIRST), "copied to the current service");
        assert_eq!(vault.load("conn-junk"), None);
        let raw = Query::generic(&throwaway.keychains(), SERVICE, Some("conn-junk")).data(false);
        assert_eq!(raw, Err(ERR_SEC_ITEM_NOT_FOUND), "nothing copied forward");
    }
}
