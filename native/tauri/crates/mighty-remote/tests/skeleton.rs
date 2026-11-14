//! W8 Step 0's seams at rest: each part's stub answers what the app expects before the part fills it (nothing connects, listens,
//! notifies, pushes or blocks), and the records written whole keep their secrets out of `Debug`.

use std::sync::Arc;

use mighty_platform::fake;
use mighty_remote::identity::{AuthDecision, AuthRefusal, Identity};
use mighty_remote::relay::connection::MobileReply;
use mighty_remote::remote::wire::RemoteHostState;

#[test]
fn an_error_reply_is_written_as_wire_json() {
    let reply = MobileReply::error(404, "Mobile route not found.", None);
    assert_eq!((reply.status, reply.body.as_str()), (404, r#"{"protocol":1,"error":"Mobile route not found."}"#));
    let coded = MobileReply::error(409, "Held \u{e9}", Some("held_not_removable"));
    assert_eq!(coded.body, r#"{"protocol":1,"error":"Held \u00E9","code":"held_not_removable"}"#);
}

#[test]
fn secrets_stay_out_of_debug() {
    let state = RemoteHostState { enabled: true, key: Some("SECRET-KEY".into()), ..RemoteHostState::default() };
    assert!(!format!("{state:?}").contains("SECRET-KEY"));
    let accepted = AuthDecision::Accepted { device_id: "d".into(), device_token: Some("SECRET-TOKEN".into()) };
    assert!(!format!("{accepted:?}").contains("SECRET-TOKEN"));
    assert_eq!(AuthRefusal::DeviceLimit.reason(), "device-limit");
}

/// W8-A: no identity is made in verification mode (the app may not write), and the store is left empty.
#[test]
fn no_identity_is_made_at_rest() {
    let pairing: Arc<dyn mighty_platform::PairingStore> = Arc::new(fake::MemoryPairing::default());
    assert!(Identity::load_or_create(pairing.clone(), false).is_err());
    assert!(pairing.load().unwrap().is_none());
}
