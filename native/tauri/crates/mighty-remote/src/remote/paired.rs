//! Remote workspaces through the relay, "페어링으로 연결 (Tailscale 없이)" (docs/remote-workspaces.md; docs/relay.md "다른 컴퓨터"): a
//! computer pairs with another computer's Mighty Code with the phone's pairing link, then reaches its remote mirror through the relay
//! the phone uses, end to end encrypted, with no Tailscale on either side.
//!
//! - On the host, [`DesktopRouter`] is the relay host's handler: a paired computer's requests (its token bound to the `desktop` role by
//!   the identity) go to the sharing host's [`RemoteHost::serve_relay`] (`/v2/info` and `/v2/mirror…`, with the same rules as over
//!   Tailscale), a phone's to the phone's router as before; neither reaches the other's routes.
//! - On the client, a paired connection is saved under a [`relay address`](relay_address) (`relay:` and the start of the host's relay
//!   name; the connections file never holds more) and its [`PairedSecret`] (the relay, the host's relay name and public key, this
//!   computer's client id and its device token, or the pairing key until the host gave a token) is the connection's key in the vault
//!   (Mac Keychain, Windows DPAPI), as a Tailscale connection's key is.

use std::sync::Arc;

use base64::Engine;
use mighty_core::json::JsonOut;
use mighty_platform::{BoxFuture, Proxy, ProxyResolver};
use serde_json::Value;
use serde_json::value::RawValue;
use zeroize::Zeroizing;

use crate::relay::client::{Credential, RelayEndpoint, decode_key, server_id};
use crate::relay::connection::{MobileDevice, MobileReply, RequestHandler};
use crate::relay::host::normalize_relay;
use crate::remote::address::RemoteAddress;
use crate::remote::host::RemoteHost;

/// A paired connection's address starts so.
pub const RELAY_ADDRESS_PREFIX: &str = "relay:";
/// How much of the host's relay name its address shows (enough to tell hosts apart; never the whole name).
const ADDRESS_NAME: usize = 16;

/// The address a paired connection is saved and shown under: `relay:` and the first 16 letters and digits of the host's relay name.
pub fn relay_address(server_id: &str) -> String {
    let name: String = server_id.chars().filter(char::is_ascii_alphanumeric).take(ADDRESS_NAME).collect();
    format!("{RELAY_ADDRESS_PREFIX}{}", name.to_ascii_lowercase())
}

/// Whether `address` is a paired connection's (`relay:` and 1 to 16 lower-case letters and digits).
pub fn is_relay_address(address: &str) -> bool {
    address
        .strip_prefix(RELAY_ADDRESS_PREFIX)
        .is_some_and(|name| (1..=ADDRESS_NAME).contains(&name.len()) && name.bytes().all(|b| b.is_ascii_digit() || b.is_ascii_lowercase()))
}

/// A saved address in its shown form: a paired connection's as it is, a Tailscale one parsed; None when it is neither.
pub fn display_address(address: &str) -> Option<String> {
    if is_relay_address(address) {
        return Some(address.to_string());
    }
    RemoteAddress::parse(address).ok().map(|parsed| parsed.display())
}

/// What a paired connection keeps in the vault: where the host is reached and how this computer proves itself. Never reaches `Debug`
/// but its relay name.
#[derive(Clone, PartialEq, Eq)]
pub struct PairedSecret {
    pub endpoint: RelayEndpoint,
    pub credential: Credential,
}

impl std::fmt::Debug for PairedSecret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PairedSecret").field("endpoint", &self.endpoint).finish_non_exhaustive()
    }
}

impl PairedSecret {
    /// `{"v":1,"relay","sid","pk","clientId","deviceToken"|"pairingKey"}`, the vault's text.
    pub fn encode(&self) -> Zeroizing<String> {
        let mut out = JsonOut::new();
        out.begin_object();
        out.prop_int("v", 1);
        out.prop_str("relay", Some(&self.endpoint.relay));
        out.prop_str("sid", Some(&self.endpoint.server_id));
        out.prop_str("pk", Some(&base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(self.endpoint.public_key)));
        out.prop_str("clientId", Some(self.credential.client_id()));
        match &self.credential {
            Credential::Token { token, .. } => out.prop_str("deviceToken", Some(token.as_str())),
            Credential::PairingKey { key, .. } => out.prop_str("pairingKey", Some(key.as_str())),
        }
        out.end_object();
        Zeroizing::new(out.finish())
    }

    /// The vault's text read back; None when it is not a paired connection's (a damaged or foreign secret: pair again).
    pub fn decode(text: &str) -> Option<PairedSecret> {
        let value: Value = serde_json::from_str(text).ok()?;
        let member = |name: &str| value.get(name).and_then(Value::as_str);
        if value.get("v").and_then(Value::as_i64) != Some(1) {
            return None;
        }
        let relay = member("relay").and_then(normalize_relay)?;
        let sid = member("sid").filter(|s| server_id(s))?.to_string();
        let public_key = member("pk").and_then(decode_key)?;
        let client_id = member("clientId").filter(|c| mighty_platform::pairing::identity_file::client_id(c))?.to_string();
        let secret = |name: &str| member(name).filter(|s| !s.is_empty() && s.encode_utf16().count() <= 256).map(str::to_string);
        let credential = match (secret("deviceToken"), secret("pairingKey")) {
            (Some(token), _) => Credential::Token { client_id, token: Zeroizing::new(token) },
            (None, Some(key)) => Credential::PairingKey { client_id, key: Zeroizing::new(key) },
            (None, None) => return None,
        };
        Some(PairedSecret { endpoint: RelayEndpoint { server_id: sid, public_key, relay }, credential })
    }
}

/// The system has no proxy for the relay: straight to it (the client's stand-in when the app gives no resolver).
pub struct DirectProxy;

impl ProxyResolver for DirectProxy {
    fn resolve(&self, _url: &str) -> BoxFuture<'_, mighty_platform::Result<Proxy>> {
        Box::pin(async { Ok(Proxy::Direct) })
    }
}

/// Finds the sharing host when a paired computer asks (None before the app made it).
pub type SharingHost = Arc<dyn Fn() -> Option<RemoteHost> + Send + Sync>;

/// The relay host's handler: a paired computer's requests to the sharing host, a phone's to the phone's router. A computer never
/// reaches a phone route and a phone never a computer's.
pub struct DesktopRouter {
    phones: Arc<dyn RequestHandler>,
    sharing: SharingHost,
}

impl DesktopRouter {
    pub fn new(phones: Arc<dyn RequestHandler>, sharing: SharingHost) -> DesktopRouter {
        DesktopRouter { phones, sharing }
    }
}

impl RequestHandler for DesktopRouter {
    fn handle(&self, device: MobileDevice, method: String, path: String, body: Option<Box<RawValue>>) -> BoxFuture<'static, MobileReply> {
        if !device.desktop {
            return self.phones.handle(device, method, path, body);
        }
        let sharing = (self.sharing)();
        Box::pin(async move {
            let Some(sharing) = sharing else {
                return error_reply(503, &mighty_run::strings::get("remote.host.error.stopped"));
            };
            sharing.serve_relay(&device.client_id, &method, &path, body.as_ref().map(|raw| raw.get().as_bytes())).await
        })
    }
}

/// `{protocol:2, error}` as the remote wire writes a refusal.
fn error_reply(status: u16, words: &str) -> MobileReply {
    let mut out = JsonOut::new();
    out.begin_object();
    out.prop_int("protocol", i64::from(crate::remote::wire::protocol::VERSION));
    out.prop_str("error", Some(words));
    out.end_object();
    MobileReply::new(status, out.finish())
}
