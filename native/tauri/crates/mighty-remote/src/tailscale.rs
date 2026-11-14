//! Tailscale's addresses (C# `TailscaleAddresses`, R\TailscaleAddresses.cs; Swift `TailscaleAddresses`; plan §1.4,
//! `native/contracts/fixtures/tailscale-addresses.json`), read from the adapters without the Tailscale CLI: Tailscale gives every
//! device an IPv4 address in 100.64.0.0/10 and an IPv6 address in `fd7a:115c:a1e0::/48`. An adapter counts as Tailscale's only when it
//! carries an address in that IPv6 range (and, where `PlatformProfile::tailscale_needs_utun`, is named `utun…`), and then its
//! addresses in both ranges count: a carrier-grade NAT can hand Wi-Fi a 100.64/10 address and other VPNs (Cloudflare WARP's
//! 100.96.0.0/12) put one on their own adapter. Windows never looks at the adapter's name (adapters are renamable, Wintun's type is
//! no named one). A mapped IPv6 counts as its IPv4; IPv4 is parsed strictly; each address once in its canonical spelling, IPv4 first,
//! each family sorted ordinally. One unreadable adapter is skipped; none readable is `failed/adaptersUnreadable`. The adapters come
//! from mighty-platform's `Network`. Owner: W8-C.

use std::net::{IpAddr, Ipv6Addr};

use mighty_platform::Adapters;

const PREFIX: [u8; 6] = [0xfd, 0x7a, 0x11, 0x5c, 0xa1, 0xe0];

/// What [`select`] found.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TailscaleSelection {
    /// This computer's Tailscale addresses, canonical, IPv4 first, sorted.
    pub addresses: Vec<IpAddr>,
    /// No adapter could be read (C# `AdaptersUnreadableException`).
    pub unreadable: bool,
}

/// `TailscaleAddresses.Select(adapters)` over a read of the port: the readable adapters' Tailscale addresses, and whether there were
/// adapters and none of them could be read.
pub fn select(adapters: &Adapters, needs_utun: bool) -> TailscaleSelection {
    let read: Vec<(String, String)> = adapters
        .adapters
        .iter()
        .flat_map(|adapter| adapter.addresses.iter().map(|address| (adapter.name.clone(), address.to_string())))
        .collect();
    let addresses = select_text(&read, needs_utun).iter().filter_map(|address| address.parse().ok()).collect();
    TailscaleSelection { addresses, unreadable: adapters.adapters.is_empty() && adapters.unreadable > 0 }
}

/// `TailscaleAddresses.IsTailscale(address)` (C# `Contains` over an address already parsed): in one of the two ranges, a mapped IPv6
/// as its IPv4.
pub fn is_tailscale(address: IpAddr) -> bool {
    match address {
        IpAddr::V4(v4) => in_cgnat(v4.octets()[0], v4.octets()[1]),
        IpAddr::V6(v6) => match v6.to_ipv4_mapped() {
            Some(mapped) => in_cgnat(mapped.octets()[0], mapped.octets()[1]),
            None => v6.octets()[..6] == PREFIX,
        },
    }
}

/// `TailscaleAddresses.Canonical(address)`: the spelling every list and comparison uses (lowercase, IPv6 compressed).
pub fn canonical(address: IpAddr) -> String {
    address.to_string()
}

/// `DirectIncoming.PeerOf`'s address: an IPv4-mapped IPv6 as its IPv4, so the per-address limits never split across forms of one
/// address.
pub fn peer(address: IpAddr) -> IpAddr {
    match address {
        IpAddr::V6(v6) => v6.to_ipv4_mapped().map_or(address, IpAddr::V4),
        v4 => v4,
    }
}

fn in_cgnat(first: u8, second: u8) -> bool {
    first == 100 && (64..=127).contains(&second)
}

fn unscoped(address: &str) -> &str {
    address.split('%').next().unwrap_or("")
}

fn trim_brackets(address: &str) -> &str {
    address.trim_matches(|c| c == '[' || c == ']')
}

/// `TailscaleAddresses.IPv4`: four dotted decimal parts, 0-255, no leading zeros (the strict form).
fn ipv4(value: &str) -> Option<[u8; 4]> {
    let parts: Vec<&str> = value.split('.').collect();
    if parts.len() != 4 {
        return None;
    }
    let mut bytes = [0u8; 4];
    for (at, part) in parts.iter().enumerate() {
        if part.is_empty() || part.len() > 3 || !part.bytes().all(|b| b.is_ascii_digit()) || (part.len() > 1 && part.starts_with('0')) {
            return None;
        }
        bytes[at] = part.parse::<u16>().ok().filter(|n| *n <= 255)? as u8;
    }
    Some(bytes)
}

fn ipv6(value: &str) -> Option<Ipv6Addr> {
    value.parse().ok()
}

/// `TailscaleAddresses.Contains(text)`: an IPv4 or IPv6 literal, optionally in brackets, in one of Tailscale's two ranges. A mapped IPv6
/// counts as its IPv4; a `%scope` never counts.
pub fn contains(address: &str) -> bool {
    let value = trim_brackets(address).to_ascii_lowercase();
    if !value.contains(':') {
        return ipv4(&value).is_some_and(|v4| in_cgnat(v4[0], v4[1]));
    }
    if value.contains('%') {
        return false;
    }
    ipv6(&value).is_some_and(|v6| is_tailscale(IpAddr::V6(v6)))
}

/// `TailscaleAddresses.InTailscaleIPv6Range`: `fd7a:115c:a1e0::/48`, which only Tailscale hands out (no `%scope`, never a mapped one).
pub fn in_tailscale_ipv6_range(address: &str) -> bool {
    let value = trim_brackets(address).to_ascii_lowercase();
    value.contains(':')
        && !value.contains('%')
        && ipv6(&value).is_some_and(|v6| v6.to_ipv4_mapped().is_none() && v6.octets()[..6] == PREFIX)
}

/// `TailscaleAddresses.Canonical(text)`: no brackets or `%scope`, lowercase, IPv6 compressed.
pub fn canonical_text(address: &str) -> String {
    let value = trim_brackets(unscoped(address)).to_ascii_lowercase();
    match value.contains(':').then(|| ipv6(&value)).flatten() {
        Some(v6) => v6.to_string(),
        None => value,
    }
}

/// `TailscaleAddresses.Owned(interfaces)`: the Tailscale addresses among `(adapter name, address as read)` pairs with each one's
/// adapter, each address once in its canonical spelling, IPv4 first, each family sorted ordinally. With `needs_utun` (the Mac) only an
/// adapter named `utun…` can be Tailscale's.
pub fn owned(interfaces: &[(String, String)], needs_utun: bool) -> Vec<(String, String)> {
    let read: Vec<(&str, &str)> = interfaces.iter().map(|(name, address)| (name.as_str(), unscoped(address))).collect();
    let tailnet: std::collections::HashSet<&str> = read
        .iter()
        .filter(|(name, address)| (!needs_utun || name.starts_with("utun")) && in_tailscale_ipv6_range(address))
        .map(|(name, _)| *name)
        .collect();
    let mut seen = std::collections::HashSet::new();
    let found: Vec<(String, String)> = read
        .iter()
        .filter(|(name, address)| tailnet.contains(name) && contains(address))
        .map(|(name, address)| (name.to_string(), canonical_text(address)))
        .filter(|(_, address)| seen.insert(address.clone()))
        .collect();
    let (mut v4, mut v6): (Vec<_>, Vec<_>) = found.into_iter().partition(|(_, address)| !address.contains(':'));
    v4.sort_by(|a, b| a.1.cmp(&b.1));
    v6.sort_by(|a, b| a.1.cmp(&b.1));
    v4.extend(v6);
    v4
}

/// `TailscaleAddresses.Select(interfaces)`: [`owned`]'s addresses.
pub fn select_text(interfaces: &[(String, String)], needs_utun: bool) -> Vec<String> {
    owned(interfaces, needs_utun).into_iter().map(|(_, address)| address).collect()
}
