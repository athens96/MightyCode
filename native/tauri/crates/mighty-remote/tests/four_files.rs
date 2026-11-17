//! The phone pairing identity over the Mac's four files (Mac stage M-B): mighty-remote's `Identity` on `mighty_platform::pairing::
//! four_files::FourFiles`, as the Mac runs it, with the Swift checks of `MobileRemoteService` and `MobileDeviceRegistry` ported by name.
//! What differs from Swift on purpose is said at each test (the shared `Identity` is C#'s: a revoke is a rotation, a client id has 22
//! characters, an unwritable list fails the pairing instead of admitting the phone without a token).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use mighty_platform::pairing::four_files::{DEVICES, FourFiles, KEY_PAIR, PAIRING_KEY, PlainFiles};
use mighty_platform::{PairingStore, PlatformError};
use mighty_remote::identity::{AuthDecision, AuthRefusal, AuthRequest, Identity, PushRegistrationResult, hash};
use serde_json::json;

const PHONE: &str = "cGhvbmUtb25lLTAwMDAwMA";
const OTHER: &str = "bmV3LWRldmljZS0wMDAwMA";
const HANDLE: &str = "__79_Pv6-fj39vX08_Lx8O_u7ezr6uno5-bl5OPi4eA";

struct Folder(PathBuf);

impl Folder {
    fn new(name: &str) -> Folder {
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        Folder(std::env::temp_dir().join(format!("mighty-remote-four-{name}-{}-{nanos}", std::process::id())).join("mobile-remote"))
    }
    fn store(&self) -> Arc<dyn PairingStore> {
        Arc::new(FourFiles::new(self.0.clone(), Box::new(PlainFiles)))
    }
    fn identity(&self) -> Identity {
        Identity::load_or_create(self.store(), true).unwrap()
    }
    /// The phone list can no longer be replaced (a folder stands in its place), as Swift's tests make the folder read-only.
    fn block_list(&self) {
        let list = self.0.join(DEVICES);
        let _ = std::fs::remove_file(&list);
        std::fs::create_dir_all(list.join("x")).unwrap();
    }
    fn unblock_list(&self) {
        let _ = std::fs::remove_dir_all(self.0.join(DEVICES));
    }
}

impl Drop for Folder {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(self.0.parent().unwrap());
    }
}

fn pair(identity: &Identity, key: &str, id: &str, name: &str) -> Result<AuthDecision, PlatformError> {
    identity.authorize(&AuthRequest::from_json(&json!({"type": "auth", "pairingKey": key, "clientId": id, "clientName": name})), true)
}

fn returning(identity: &Identity, id: &str, token: &str) -> AuthDecision {
    identity.authorize(&AuthRequest::from_json(&json!({"type": "auth", "deviceToken": token, "clientId": id})), true).unwrap()
}

fn token(decision: AuthDecision) -> String {
    match decision {
        AuthDecision::Accepted { device_token: Some(token), .. } => token,
        other => panic!("no token: {other:?}"),
    }
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap()
}

/// `MobileRemoteTests.swift:895 "revokingRotatesTheKeyBeforeTheRowAndFailsWholeOrNotAtAll"`: a revoke is a rotation (the shared
/// `Identity`'s, C#'s: every phone pairs again); one that cannot be written leaves the key and the phone as they were, on disk too.
#[test]
fn revoking_rotates_the_key_before_the_row_and_fails_whole_or_not_at_all() {
    let folder = Folder::new("revoke");
    let identity = folder.identity();
    let key = identity.pairing_key().to_string();
    let phone = token(pair(&identity, &key, PHONE, "iPhone").unwrap());
    folder.block_list();
    assert!(identity.rotate().is_err(), "half a revoke is never reported as a whole one");
    assert_eq!(identity.devices().iter().map(|d| d.id.as_str()).collect::<Vec<_>>(), [PHONE]);
    assert_eq!(*identity.pairing_key(), key);
    assert_eq!(read(&folder.0.join(PAIRING_KEY)), key, "the key written first was put back");
    folder.unblock_list();
    identity.rotate().unwrap();
    assert!(identity.devices().is_empty() && *identity.pairing_key() != key);
    assert_eq!(returning(&identity, PHONE, &phone), AuthDecision::Refused(AuthRefusal::DeviceRevoked));
    let reloaded = folder.identity();
    assert!(reloaded.devices().is_empty() && *reloaded.pairing_key() == *identity.pairing_key());
}

/// `MobileRemoteTests.swift:944 "keysPersistAndStatusExposesTheOfferOnlyWhileConnected"` (the keys' half; the offer while connected is
/// the relay's): the key, the host token (the relay's server id) and the key pair survive a new identity over the same files, and a
/// rotation empties the list.
#[test]
fn keys_persist_and_status_exposes_the_offer_only_while_connected() {
    let folder = Folder::new("persist");
    let first = folder.identity();
    let again = folder.identity();
    assert_eq!(
        (first.pairing_key().to_string(), first.server_id(), first.public_key()),
        (again.pairing_key().to_string(), again.server_id(), again.public_key())
    );
    assert!(read(&folder.0.join(KEY_PAIR)).starts_with("{\"publicKeyB64\":"), "Swift's key pair file");
    let key = first.pairing_key().to_string();
    token(pair(&first, &key, PHONE, "iPhone").unwrap());
    first.rotate().unwrap();
    assert!(folder.identity().devices().is_empty() && *folder.identity().pairing_key() != key);
    assert_eq!(folder.identity().server_id(), again.server_id(), "the server id is the host token's, untouched by a rotation");
}

/// `RelayChannelTests.swift:34 "keypairPersistsOwnerOnlyAndPairingOfferRoundTrips"` (the key pair's half; its modes are
/// mighty-platform-macos `pairing.rs`'s): the key pair is the same on every load, a damaged file gets a new one, and the pairing link
/// carries the public key it gives.
#[test]
fn keypair_persists_owner_only_and_pairing_offer_round_trips() {
    let folder = Folder::new("keypair");
    let first = folder.identity().public_key();
    assert_eq!(folder.identity().public_key(), first);
    std::fs::write(folder.0.join(KEY_PAIR), "garbage").unwrap();
    let mended = folder.identity();
    assert_ne!(mended.public_key(), first);
    let link = mended.pairing_url("wss://relay.example.com:8443", "Young's Mac", &[]);
    assert!(link.starts_with(&format!("mightycode://pair?v=2&sid={}&pk=", mended.server_id())), "{link}");
    assert!(!link.contains('+') && !link.contains("/ws"));
}

/// `MobileRemoteTests.swift:1033 "aPhoneWithATokenRegistersAHandleAndTheKindsItWants"`: a phone with a token registers a handle and
/// its kinds (an unknown kind left out), kept in the list on disk; unregistering clears it there too.
#[test]
fn a_phone_with_a_token_registers_a_handle_and_the_kinds_it_wants() {
    let folder = Folder::new("push");
    let identity = folder.identity();
    let key = identity.pairing_key().to_string();
    token(pair(&identity, &key, PHONE, "Phone").unwrap());
    let kinds = ["permission".to_string(), "completed".to_string(), "future-kind".to_string()];
    assert_eq!(identity.set_push(PHONE, HANDLE, &kinds), PushRegistrationResult::Saved);
    let pushed = folder.identity().push_devices();
    assert_eq!(pushed.len(), 1);
    assert_eq!((pushed[0].device_id.as_str(), pushed[0].handle.as_str()), (PHONE, HANDLE));
    let mut wanted = pushed[0].kinds.clone();
    wanted.sort();
    assert_eq!(wanted, ["completed", "permission"]);
    assert!(identity.clear_push(PHONE, None));
    assert!(folder.identity().push_devices().is_empty());
}

/// `MobileRemoteTests.swift:1066 "anUnregisterThatCannotBeSavedIsReportedAndKeepsTheHandle"`.
#[test]
fn an_unregister_that_cannot_be_saved_is_reported_and_keeps_the_handle() {
    let folder = Folder::new("unregister");
    let identity = folder.identity();
    let key = identity.pairing_key().to_string();
    token(pair(&identity, &key, PHONE, "Phone").unwrap());
    assert_eq!(identity.set_push(PHONE, HANDLE, &["completed".to_string()]), PushRegistrationResult::Saved);
    folder.block_list();
    assert!(!identity.clear_push(PHONE, None), "a phone told 'off' hears that it failed");
    assert_eq!(identity.push_devices().iter().map(|d| d.handle.as_str()).collect::<Vec<_>>(), [HANDLE]);
    folder.unblock_list();
    assert!(identity.clear_push(PHONE, None));
    assert!(identity.clear_push(PHONE, None), "nothing left to clear is not a failure");
    assert!(folder.identity().push_devices().is_empty());
}

/// `MobileRemoteTests.swift:1107 "theListDropsAHandleItCannotUse"`: a Swift-written list whose rows carry handles they cannot use
/// loses the handles and keeps the phones.
#[test]
fn the_list_drops_a_handle_it_cannot_use() {
    let folder = Folder::new("handles");
    folder.identity();
    let stamp = "2026-10-23T00:00:00.000Z";
    let hash_a = "a".repeat(64);
    let rows = json!([
        {"id": PHONE, "name": "Bad handle", "tokenHash": hash("x"), "firstSeen": stamp, "lastSeen": stamp,
         "push": {"handle": "not-a-handle", "kinds": ["completed"], "updatedAt": stamp}},
        {"id": "legacy", "name": "Legacy", "firstSeen": stamp, "lastSeen": stamp, "push": {"handle": HANDLE, "kinds": ["completed"], "updatedAt": stamp}},
        {"id": OTHER, "name": "Good", "tokenHash": hash_a, "firstSeen": stamp, "lastSeen": stamp,
         "push": {"handle": HANDLE, "kinds": ["completed", "future-kind"], "updatedAt": stamp}},
    ]);
    std::fs::write(folder.0.join(DEVICES), rows.to_string()).unwrap();
    let identity = folder.identity();
    assert_eq!(identity.devices().iter().map(|d| d.id.as_str()).collect::<Vec<_>>(), [PHONE, "legacy", OTHER]);
    let pushed = identity.push_devices();
    assert_eq!(pushed.len(), 1);
    assert_eq!((pushed[0].device_id.as_str(), pushed[0].kinds.clone()), (OTHER, vec!["completed".to_string()]));
}

/// `MobileRemoteExtensionTests.swift:1148 "aDeviceTokenIsIssuedOnceAndOnlyItsHashIsKept"` (Swift's removal of one row is a rotation in
/// the shared `Identity`): the token is handed out once, only its hash is on disk, and an identity over the same files admits it.
#[test]
fn a_device_token_is_issued_once_and_only_its_hash_is_kept() {
    let folder = Folder::new("token");
    let identity = folder.identity();
    let key = identity.pairing_key().to_string();
    let phone = token(pair(&identity, &key, PHONE, "Young's iPhone").unwrap());
    assert_eq!(identity.devices().len(), 1);
    assert!(!identity.devices()[0].legacy && identity.devices()[0].name == "Young's iPhone");
    assert!(matches!(returning(&identity, PHONE, &phone), AuthDecision::Accepted { device_token: None, .. }));
    let wrong = format!("{}{}", &phone[..phone.len() - 1], if phone.ends_with('A') { "B" } else { "A" });
    assert_eq!(returning(&identity, PHONE, &wrong), AuthDecision::Refused(AuthRefusal::DeviceRevoked));
    assert_eq!(returning(&identity, OTHER, &phone), AuthDecision::Refused(AuthRefusal::DeviceRevoked));
    let saved = read(&folder.0.join(DEVICES));
    assert!(!saved.contains(&phone) && saved.contains(&hash(&phone)), "the token never touches the disk; its hash does");
    assert!(matches!(returning(&folder.identity(), PHONE, &phone), AuthDecision::Accepted { .. }));
}

/// `MobileRemoteExtensionTests.swift:1179 "pairingAgainWithTheKeyGivesTheSameRowANewToken"`.
#[test]
fn pairing_again_with_the_key_gives_the_same_row_a_new_token() {
    let folder = Folder::new("again");
    let identity = folder.identity();
    let key = identity.pairing_key().to_string();
    let first = token(pair(&identity, &key, PHONE, "Young's iPhone").unwrap());
    let before = folder.identity().devices()[0].first_seen.clone();
    let fresh = token(pair(&identity, &key, PHONE, "Young's Galaxy").unwrap());
    assert_ne!(fresh, first);
    let after = folder.identity().devices();
    assert!(after.len() == 1 && after[0].first_seen == before && after[0].name == "Young's Galaxy");
    assert!(matches!(returning(&identity, PHONE, &fresh), AuthDecision::Accepted { .. }));
    assert_eq!(returning(&identity, PHONE, &first), AuthDecision::Refused(AuthRefusal::DeviceRevoked));
    assert_eq!(pair(&identity, "not-the-key", PHONE, "x").unwrap(), AuthDecision::Refused(AuthRefusal::PairingKey));
}

/// `MobileRemoteExtensionTests.swift:1205 "aFailedWriteIssuesNoTokenAndMakesARevokeFail"` (the shared `Identity` refuses the pairing
/// where Swift admitted the phone without a token: C#'s `Save` throws): no token, the list exactly as it was, and a revoke fails.
#[test]
fn a_failed_write_issues_no_token_and_makes_a_revoke_fail() {
    let folder = Folder::new("failed");
    let identity = folder.identity();
    let key = identity.pairing_key().to_string();
    let phone = token(pair(&identity, &key, PHONE, "iPhone").unwrap());
    let before = identity.devices();
    folder.block_list();
    assert!(pair(&identity, &key, OTHER, "new").is_err(), "a token the host cannot remember is worse than no token");
    assert_eq!(identity.devices(), before);
    assert!(identity.rotate().is_err());
    assert_eq!(identity.devices(), before);
    assert!(matches!(returning(&identity, PHONE, &phone), AuthDecision::Accepted { .. }));
    folder.unblock_list();
    identity.rotate().unwrap();
    assert!(identity.devices().is_empty());
}
