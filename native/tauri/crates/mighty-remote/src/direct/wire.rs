//! The direct path's constants (C# `MobileDirect`, R\MobileDirect.cs; docs/relay.md "Tailscale 직접 연결"; plan §1.4): a phone on the
//! same tailnet opens its data socket straight to this PC at plain `ws://<endpoint>/ws?v=1` instead of to the relay; everything after
//! the upgrade is the relay path unchanged (the same hello/ready handshake, ChaCha20-Poly1305 frames and auth, the phone pinning the
//! host key). Port 43139 (1024-65535), one listener per Tailscale address with backlog 36, at most 8 endpoints advertised (IPv6 in
//! brackets), connection ids `direct-…`. Owner: W8-C.

use std::time::Duration;

/// `MobileDirect.DefaultPort` (mighty-core's `MOBILE_DIRECT_PORT`).
pub const DEFAULT_PORT: u16 = mighty_core::model::MOBILE_DIRECT_PORT as u16;
/// The frame limit on both paths: the relay's own, kept for the direct socket that replaces it.
pub const MAXIMUM_FRAME_BYTES: usize = 1024 * 1024;
/// The phone table, shared with the relay path (docs/relay.md: 32 per host).
pub const MAXIMUM_CLIENTS: usize = 32;
/// Connections the listeners hold together: four in their handshake and the 32 phones (C# `ConnectionLimit`); the listen backlog too.
pub const CONNECTION_LIMIT: usize = super::admit::MAXIMUM_UNAUTHENTICATED + MAXIMUM_CLIENTS;
pub const BACKLOG: u32 = CONNECTION_LIMIT as u32;
pub const MAXIMUM_ENDPOINTS: usize = 8;
/// One deadline covers the socket, the upgrade, hello and auth.
pub const HANDSHAKE_DEADLINE: Duration = Duration::from_secs(10);
/// Tailscale's addresses are looked at again this often (and on every address change).
pub const REDETECT_INTERVAL: Duration = Duration::from_secs(15);
/// A direct connection's id starts so; relay notices never open or close one.
pub const CONNECTION_PREFIX: &str = "direct-";
/// A direct refusal says one word, so a tailnet peer learns nothing of which ids or keys this PC knows (auth_error's reason).
pub const GENERIC_REFUSAL: &str = "unauthorized";

/// `MobileDirect.ValidPort`.
pub fn valid_port(port: i64) -> bool {
    (1024..=65535).contains(&port)
}

/// `MobileDirect.IsDirect`.
pub fn is_direct(connection_id: &str) -> bool {
    connection_id.starts_with(CONNECTION_PREFIX)
}

/// `MobileDirect.Endpoint`: `host:port`, an IPv6 literal in brackets.
pub fn endpoint(host: &str, port: u16) -> String {
    if host.contains(':') { format!("[{host}]:{port}") } else { format!("{host}:{port}") }
}

/// Swift `MobileDirect.isTailnetName`: a MagicDNS name, lower-case DNS labels under `ts.net` (253 characters at most).
pub fn is_tailnet_name(text: &str) -> bool {
    let Some(labels) = text.strip_suffix(".ts.net") else { return false };
    let label = |label: &str| {
        let bytes = label.as_bytes();
        (1..=63).contains(&bytes.len())
            && bytes.iter().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || *b == b'-')
            && bytes[0] != b'-'
            && bytes[bytes.len() - 1] != b'-'
    };
    text.len() <= 253 && !labels.is_empty() && labels.split('.').all(label)
}

/// Where Swift's `MobileDirect.magicDNSName` looks for the Tailscale CLI: the app's and Homebrew's places, then every absolute entry of
/// `path` (a relative one could run a `tailscale` from the working folder), each once.
pub fn tailscale_candidates(path: &str) -> Vec<String> {
    let mut candidates: Vec<String> =
        ["/Applications/Tailscale.app/Contents/MacOS/Tailscale", "/opt/homebrew/bin/tailscale", "/usr/local/bin/tailscale"]
            .into_iter()
            .map(str::to_string)
            .collect();
    for folder in path.split(':').filter(|folder| folder.starts_with('/')) {
        let candidate = format!("{}/tailscale", folder.trim_end_matches('/'));
        if !candidates.contains(&candidate) {
            candidates.push(candidate);
        }
    }
    candidates
}

/// `tailscale status --json`'s own node's `DNSName`, lower case, its dots trimmed; None when the answer holds none (Swift tries the
/// next candidate then). Whether it is a MagicDNS name is [`is_tailnet_name`]'s.
pub fn status_dns_name(status_json: &[u8]) -> Option<String> {
    let value: serde_json::Value = serde_json::from_slice(status_json).ok()?;
    let raw = value.get("Self")?.get("DNSName")?.as_str()?;
    Some(raw.to_lowercase().trim_matches('.').to_string())
}

/// Swift `MobileDirect.advertised(bound:name:port:allowAnyHost:)`: the bound addresses with their ports, then the MagicDNS `name` on
/// `port` while anything is bound (`PlatformProfile::direct_advertises_magic_dns`; Windows passes none); only Tailscale addresses and
/// MagicDNS names (any host through the checks' seam), each once, at most eight.
pub fn advertised_with_name(bound: &[(String, u16)], name: Option<&str>, port: u16, allow_any_host: bool) -> Vec<String> {
    let mut hosts: Vec<(String, u16)> = bound.to_vec();
    if let Some(name) = name.filter(|_| !bound.is_empty()) {
        hosts.push((name.to_string(), port));
    }
    let mut seen = std::collections::HashSet::new();
    hosts
        .iter()
        .filter(|(host, _)| allow_any_host || crate::tailscale::contains(host) || is_tailnet_name(host))
        .map(|(host, port)| endpoint(host, *port))
        .filter(|endpoint| seen.insert(endpoint.clone()))
        .take(MAXIMUM_ENDPOINTS)
        .collect()
}

/// `MobileDirect.Advertised`: the bound addresses with their ports, Tailscale addresses only (any host through the checks' seam), each
/// once, at most eight.
pub fn advertised(bound: &[(String, u16)], allow_any_host: bool) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    bound
        .iter()
        .filter(|(host, _)| allow_any_host || crate::tailscale::contains(host))
        .map(|(host, port)| endpoint(host, *port))
        .filter(|endpoint| seen.insert(endpoint.clone()))
        .take(MAXIMUM_ENDPOINTS)
        .collect()
}
