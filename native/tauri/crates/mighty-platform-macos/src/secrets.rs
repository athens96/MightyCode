//! M-B: remote workspaces' keys in the Keychain. Swift `MightyCore/Remote/RemoteKeychain.swift` (49 lines): a generic password of
//! service `dev.mightycode.native.remote` (a key found only under the legacy `dev.mightyclaude.native.remote` copied forward when
//! first read), account = the connection's id, `kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly`; contract `mighty_platform::secrets`
//! (`SecretVault`). The Keychain queries here are shared with `claude_credentials.rs`.
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

    /// `SecItemCopyMatching` for the item's data (`kSecMatchLimitOne`); `silent` adds `kSecUseAuthenticationUIFail`.
    pub(crate) fn data(self, silent: bool) -> std::result::Result<Vec<u8>, i32> {
        let mut query = self
            .with(unsafe { kSecReturnData }, CFBoolean::true_value().as_CFType())
            .with(unsafe { kSecMatchLimit }, key(unsafe { kSecMatchLimitOne }).as_CFType());
        if silent {
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

/// The remote keys' Keychain.
pub struct KeychainVault {
    keychains: Keychains,
}

/// Swift `RemoteKeychain.service`.
pub const SERVICE: &str = "dev.mightycode.native.remote";
/// Swift `RemoteKeychain.legacyService`: where the app kept the keys before it was renamed to Mighty Code.
pub const LEGACY_SERVICE: &str = "dev.mightyclaude.native.remote";

impl KeychainVault {
    /// The user's default keychains (the login keychain).
    pub fn user() -> KeychainVault {
        KeychainVault { keychains: Keychains { only: None } }
    }

    /// One keychain only (a test's throwaway file keychain).
    pub fn only(keychain: security_framework::os::macos::keychain::SecKeychain) -> KeychainVault {
        KeychainVault { keychains: Keychains { only: Some(keychain) } }
    }

    fn read(&self, id: &str, service: &str) -> Option<String> {
        let bytes = Query::generic(&self.keychains, service, Some(id)).data(false).ok()?;
        String::from_utf8(bytes).ok()
    }

    /// `SecItemUpdate`, else `SecItemAdd` with `kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly` (into the confined keychain).
    fn write(&self, id: &str, secret: &str) -> std::result::Result<(), i32> {
        let query = Query::generic(&self.keychains, SERVICE, Some(id));
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
        let mut item = Query::generic(&Keychains { only: None }, SERVICE, Some(id))
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
        if let Some(secret) = self.read(id, SERVICE) {
            return Some(secret);
        }
        let secret = self.read(id, LEGACY_SERVICE)?;
        let _ = self.write(id, &secret);
        Some(secret)
    }

    fn save(&self, id: &str, secret: &str) -> Result<()> {
        // The status names the failure, never the secret or the id.
        self.write(id, secret).map_err(|status| PlatformError::Refused(format!("the Keychain refused the key (OSStatus {status})")))
    }

    fn delete(&self, id: &str) -> Result<()> {
        let query = Query::generic(&self.keychains, SERVICE, Some(id));
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
    use super::*;

    /// New, with no Swift test (none exists for `RemoteKeychain`): a key saved, replaced, read back and deleted, in a throwaway file
    /// keychain; a delete of a missing key succeeds.
    #[test]
    fn remote_keys_round_trip_in_a_throwaway_keychain() {
        let throwaway = Throwaway::new("remote-keys");
        let vault = KeychainVault::only(throwaway.keychain.clone());
        assert_eq!(vault.load("conn-1"), None);
        vault.save("conn-1", "first-secret").unwrap();
        assert_eq!(vault.load("conn-1").as_deref(), Some("first-secret"));
        vault.save("conn-1", "second-secret").unwrap();
        assert_eq!(vault.load("conn-1").as_deref(), Some("second-secret"));
        vault.delete("conn-1").unwrap();
        vault.delete("conn-1").unwrap();
        assert_eq!(vault.load("conn-1"), None);
    }

    /// New, with no Swift test: a key found only under the pre-rename service is answered and copied to the current one.
    #[test]
    fn a_legacy_service_item_is_copied_forward_on_read() {
        let throwaway = Throwaway::new("remote-legacy");
        throwaway.add(LEGACY_SERVICE, "conn-old", b"old-secret");
        let vault = KeychainVault::only(throwaway.keychain.clone());
        assert_eq!(vault.load("conn-old").as_deref(), Some("old-secret"));
        assert_eq!(vault.read("conn-old", SERVICE).as_deref(), Some("old-secret"), "copied to the current service");
    }
}
