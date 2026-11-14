//! Tailscale's addresses (C# `TailscaleAddresses`, R\TailscaleAddresses.cs; plan §1.4,
//! `native/contracts/fixtures/tailscale-addresses.json`):
//! 100.64/10 and `fd7a:115c:a1e0::/48` (a mapped IPv6 counts as its IPv4, IPv4 parsed strictly); an adapter counts only when it
//! carries an fd7a address (and, where `PlatformProfile::tailscale_needs_utun`, is named `utun…`); canonical spelling, IPv4 first,
//! ordinal sort; one unreadable adapter skipped, none readable `failed/adaptersUnreadable`. The adapters come from mighty-platform's
//! `Network`. Owner: W8-C.
//!
//! At rest (W8 Step 0): no address is Tailscale's.

use std::net::IpAddr;

use mighty_platform::Adapters;

/// What [`select`] found.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TailscaleSelection {
    /// This computer's Tailscale addresses, canonical, IPv4 first, sorted.
    pub addresses: Vec<IpAddr>,
    /// No adapter could be read.
    pub unreadable: bool,
}

/// `TailscaleAddresses.Select(adapters)`.
pub fn select(adapters: &Adapters, needs_utun: bool) -> TailscaleSelection {
    let _ = (adapters, needs_utun);
    TailscaleSelection::default()
}

/// `TailscaleAddresses.IsTailscale(address)`.
pub fn is_tailscale(address: IpAddr) -> bool {
    let _ = address;
    false
}

/// `TailscaleAddresses.Canonical(address)`: the spelling every list and comparison uses.
pub fn canonical(address: IpAddr) -> String {
    address.to_string()
}
