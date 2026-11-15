//! A remote workspace's address (C# `RemoteAddress`, `RemotePeerPolicy`, `RemoteRoute`, R\RemoteAddress.cs; plan §1.6): `Parse`
//! (default port 43137), the peer policy (Tailscale only; loopback only where a check or the smoke injects it in code), and the route
//! probe (a UDP connect showing the route leaves this PC's Tailscale address, else `notViaTailscale`). The address rules of
//! `TailscaleAddresses` these need (`Contains`, the strict IPv4 form, `Canonical`) are W8-C's [`crate::tailscale`]. Owner: W8-D.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, UdpSocket};

use mighty_run::strings::get;

use crate::tailscale;

pub const DEFAULT_PORT: u16 = crate::remote::wire::protocol::DEFAULT_PORT;

/// C# `RemoteAddress(Host, Port)`: a Tailscale IP or a MagicDNS name and its port.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RemoteAddress {
    pub host: String,
    pub port: u16,
}

/// `RemoteAddress.MaximumInput`.
pub const MAXIMUM_INPUT: usize = 300;

impl RemoteAddress {
    /// `Display`: host:port, IPv6 in brackets: what is saved and shown.
    pub fn display(&self) -> String {
        if self.host.contains(':') { format!("[{}]:{}", self.host, self.port) } else { format!("{}:{}", self.host, self.port) }
    }

    /// `RemoteAddress.Parse(input)`: an optional `http://`, a host, an optional port; Err with `remote.connect.error.badAddress` or
    /// `.badPort` when it is no host address.
    pub fn parse(input: &str) -> Result<RemoteAddress, String> {
        let bad = || get("remote.connect.error.badAddress");
        if input.contains('\0') {
            return Err(bad());
        }
        // Whitespace and newlines around the address are trimmed (char.IsWhiteSpace); a control character anywhere is refused below.
        let mut text = input.trim_matches(char::is_whitespace);
        let length = text.encode_utf16().count();
        if length == 0 || length > MAXIMUM_INPUT {
            return Err(bad());
        }
        if text.len() >= 7 && text.is_char_boundary(7) && text[..7].eq_ignore_ascii_case("http://") {
            text = &text[7..];
        }
        if let Some(stripped) = text.strip_suffix('/') {
            text = stripped;
        }
        if text.contains(['/', '@', '?', '#', '%']) || text.contains("://") {
            return Err(bad());
        }
        let mut host = text.to_string();
        let mut port: i64 = DEFAULT_PORT.into();
        if text.starts_with('[') {
            let close = text.find(']').ok_or_else(bad)?;
            host = text[1..close].to_string();
            let rest = &text[close + 1..];
            if !rest.is_empty() {
                port = rest.strip_prefix(':').and_then(read_port).ok_or_else(bad)?;
            }
        } else if text.matches(':').count() == 1 {
            let colon = text.find(':').unwrap_or_default();
            host = text[..colon].to_string();
            port = read_port(&text[colon + 1..]).ok_or_else(bad)?;
        }
        let host = host.to_lowercase();
        if !(1024..=65535).contains(&port) {
            return Err(get("remote.connect.error.badPort"));
        }
        let characters =
            !host.is_empty() && host.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'.' | b':' | b'-'));
        if host.is_empty() || host.len() > 253 || !characters || host.starts_with('-') || host.starts_with('.') || host.contains("..") {
            return Err(bad());
        }
        // A colon is only ever an IPv6 literal's.
        let host = if host.contains(':') {
            host.parse::<Ipv6Addr>().map_err(|_| bad())?;
            tailscale::canonical_text(&host)
        } else {
            host
        };
        Ok(RemoteAddress { host, port: port as u16 })
    }
}

/// As Swift's `Int(_:)` reads a port: ASCII digits after an optional + or -, so "+5000" is 5000 and "-1" a port out of range.
fn read_port(value: &str) -> Option<i64> {
    let digits = value.strip_prefix(['+', '-']).unwrap_or(value);
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let number: i64 = digits.parse().ok().filter(|n| *n <= i64::from(i32::MAX))?;
    Some(if value.starts_with('-') { -number } else { number })
}

/// C# `RemotePeerPolicy.Allowed(address, allowLoopback)`: a Tailscale address; loopback only when injected, in the spellings the Mac's
/// `inet_pton` check takes (four dotted decimal parts, or `::1` exactly).
pub fn peer_allowed(address: &str, allow_loopback: bool) -> bool {
    // A peer's address may carry its interface (fd7a:…%17); the address alone decides.
    let unscoped = address.split('%').next().unwrap_or_default();
    if tailscale::contains(unscoped) {
        return true;
    }
    if !allow_loopback {
        return false;
    }
    let value = unscoped.trim_matches(['[', ']']).to_lowercase();
    match tailscale::ipv4(&value) {
        Some(v4) => v4[0] == 127,
        None => value == "::1",
    }
}

/// `RemotePeerPolicy.Allowed(IPAddress)`: a peer as the socket reports it (an IPv4-mapped address as its IPv4).
pub fn peer_ip_allowed(address: IpAddr, allow_loopback: bool) -> bool {
    peer_allowed(&tailscale::peer(address).to_string(), allow_loopback)
}

/// `RemoteRoute.SourceAddressToward(ip)`: the local address the routing table picks toward `ip` (a literal, optionally in brackets),
/// canonical. A UDP socket is connected, which only picks the route; nothing is sent. None for no IP or no route.
pub fn source_address_toward(ip: &str) -> Option<String> {
    let target: IpAddr = ip.trim_matches(['[', ']']).parse().ok()?;
    let any = match target {
        IpAddr::V4(_) => SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), 0),
        IpAddr::V6(_) => SocketAddr::new(IpAddr::V6(Ipv6Addr::UNSPECIFIED), 0),
    };
    let socket = UdpSocket::bind(any).ok()?;
    socket.connect(SocketAddr::new(target, 9)).ok()?;
    let local = socket.local_addr().ok()?;
    Some(tailscale::canonical_text(&tailscale::peer(local.ip()).to_string()))
}

/// `RemoteRoute.ThroughTailscale(ip, current)`: the route toward `ip` leaves from one of this PC's Tailscale addresses (`current`, Err
/// when the adapters cannot be read). No route, or unreadable adapters, is a no.
pub fn through_tailscale(ip: &str, current: &dyn Fn() -> Result<Vec<String>, String>) -> bool {
    let Some(source) = source_address_toward(ip) else { return false };
    current().is_ok_and(|addresses| addresses.contains(&source))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn displays_put_ipv6_in_brackets() {
        assert_eq!(RemoteAddress { host: "fd7a:115c:a1e0::1".into(), port: 43137 }.display(), "[fd7a:115c:a1e0::1]:43137");
        assert_eq!(RemoteAddress { host: "100.64.0.1".into(), port: 5000 }.display(), "100.64.0.1:5000");
    }
}
