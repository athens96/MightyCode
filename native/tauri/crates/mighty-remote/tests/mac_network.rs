//! The Mac's network checks named in the Mac plan (M-C), each under its Swift name: which interface counts as Tailscale's on a Mac
//! (`PlatformProfile::tailscale_needs_utun`, the fixture's `darwin` column) and when the relay link redials as the Mac's paths change
//! (a `utun` VPN coming up, an on-demand tunnel alone). The rules are shared code, so they run on every OS over the Mac's value; the
//! checks that read this machine run on the Mac through its own network port (`getifaddrs`).

use std::net::IpAddr;
use std::path::Path;

use mighty_platform::{Adapter, Adapters};
use mighty_remote::relay::policy::{self, LinkAction, NetworkPath};
use mighty_remote::tailscale::{self, contains, select_text};
use serde_json::Value;

fn pairs(items: &[(&str, &str)]) -> Vec<(String, String)> {
    items.iter().map(|(name, address)| (name.to_string(), address.to_string())).collect()
}

fn adapter(name: &str, addresses: &[&str]) -> Adapter {
    Adapter { name: name.into(), addresses: addresses.iter().map(|a| a.parse::<IpAddr>().unwrap()).collect() }
}

/// `RemoteWorkspaceTests.swift:41 "onlyATunnelCarryingTailscalesIPv6PrefixCountsAndOnlyItsAddressesAreKept"`, under the Mac's rule
/// (`needs_utun`).
#[test]
fn only_a_tunnel_carrying_tailscales_ipv6_prefix_counts_and_only_its_addresses_are_kept() {
    let interfaces = pairs(&[
        ("lo0", "127.0.0.1"),
        ("lo0", "::1"),
        ("en0", "192.168.0.5"),
        ("en0", "fe80::1c2b:3d4e:5f60:7182%en0"),
        // A hotspot handing out CGNAT addresses on Wi-Fi is not Tailscale.
        ("en0", "100.70.1.2"),
        // WARP and other VPNs put 100.64/10 on a tunnel too, but not fd7a:115c:a1e0::/48.
        ("utun2", "100.96.0.2"),
        ("utun2", "fd00:abcd::2"),
        ("utun3", "10.8.0.2"),
        // Tailscale: its tunnel carries both ranges.
        ("utun4", "fe80::abcd%utun4"),
        ("utun4", "fd7a:115c:a1e0::1234:5678"),
        ("utun4", "100.101.102.103"),
        ("utun4", "100.128.0.1"),
        ("utun4", "10.0.0.9"),
    ]);
    assert_eq!(select_text(&interfaces, true), ["100.101.102.103", "fd7a:115c:a1e0::1234:5678"]);
    assert_eq!(tailscale::owned(&interfaces, true).iter().map(|(name, _)| name.as_str()).collect::<Vec<_>>(), ["utun4", "utun4"]);
    assert!(select_text(&pairs(&[("utun7", "100.101.102.103")]), true).is_empty(), "the CGNAT address alone is never taken");
    assert_eq!(select_text(&pairs(&[("utun7", "fd7a:115c:a1e0::9")]), true), ["fd7a:115c:a1e0::9"], "the IPv6 prefix alone is enough");
    assert!(
        select_text(&pairs(&[("en0", "fd7a:115c:a1e0::9"), ("en0", "100.101.102.103")]), true).is_empty(),
        "the prefix on an interface that is not a tunnel does not count"
    );
    assert_eq!(
        select_text(&pairs(&[("utun7", "fd7a:115c:a1e0::9%utun7"), ("utun7", "100.101.102.103")]), true),
        ["100.101.102.103", "fd7a:115c:a1e0::9"],
        "an address read with its %scope is taken without it"
    );
    assert_eq!(
        select_text(
            &pairs(&[("utun7", "fd7a:115c:a1e0::b"), ("utun7", "100.101.1.2"), ("utun7", "fd7a:115c:a1e0::a"), ("utun7", "100.100.1.2")]),
            true
        ),
        ["100.100.1.2", "100.101.1.2", "fd7a:115c:a1e0::a", "fd7a:115c:a1e0::b"],
        "each family sorted, whatever order the interfaces were read in"
    );
    assert!(contains("100.64.0.1") && contains("fd7a:115c:a1e0::1") && !contains("192.168.0.5") && !contains("127.0.0.1"), "a peer");
    // Whatever this Mac has, detection never offers loopback or a LAN address: only a `utun`'s Tailscale addresses.
    #[cfg(target_os = "macos")]
    {
        use mighty_platform::Network as _;
        let read = mighty_platform_macos::net::MacNetwork.adapters().unwrap();
        let flat: Vec<(String, String)> =
            read.adapters.iter().flat_map(|a| a.addresses.iter().map(|ip| (a.name.clone(), ip.to_string()))).collect();
        for (name, address) in tailscale::owned(&flat, true) {
            assert!(name.starts_with("utun") && contains(&address), "{name} {address}");
        }
    }
}

/// `RemoteWorkspaceTests.swift:736 "everyInterfaceListSelectsTheMacsExpectedAddresses"`: every case of the shared fixture names both
/// platforms, and the Mac's rule selects its `darwin` answer.
#[test]
fn every_interface_list_selects_the_macs_expected_addresses() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../contracts/fixtures/tailscale-addresses.json");
    let root: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    let cases = root["cases"].as_array().unwrap();
    assert!(cases.len() >= 12);
    let strings = |value: &Value| value.as_array().unwrap().iter().map(|v| v.as_str().unwrap().to_string()).collect::<Vec<_>>();
    for case in cases {
        let name = case["name"].as_str().unwrap();
        assert!(case["expected"]["darwin"].is_array() && case["expected"]["win32"].is_array(), "{name} names both platforms");
        let read: Vec<(String, String)> = case["interfaces"]
            .as_array()
            .unwrap()
            .iter()
            .flat_map(|interface| {
                let name = interface["name"].as_str().unwrap().to_string();
                strings(&interface["addresses"]).into_iter().map(move |address| (name.clone(), address))
            })
            .collect();
        assert_eq!(select_text(&read, true), strings(&case["expected"]["darwin"]), "{name}");
    }
}

/// The Mac's paths as the network port reads them (`NetworkPath::from_adapters`): Wi-Fi on one router, on another, and with Tailscale's
/// `utun4` up beside it (RelayLinkTests.swift:9-11's `wifi`, `otherWifi`, `vpn`).
fn paths() -> (NetworkPath, NetworkPath, NetworkPath) {
    let wifi = NetworkPath::from_adapters(&Adapters { adapters: vec![adapter("en0", &["192.168.0.5"])], unreadable: 0 });
    let other = NetworkPath::from_adapters(&Adapters { adapters: vec![adapter("en0", &["10.0.0.5"])], unreadable: 0 });
    let vpn = NetworkPath::from_adapters(&Adapters {
        adapters: vec![adapter("lo0", &["127.0.0.1"]), adapter("en0", &["192.168.0.5"]), adapter("utun4", &["100.101.102.103", "fe80::1"])],
        unreadable: 0,
    });
    (wifi, other, vpn)
}

/// `RelayLinkTests.swift:19 "aDifferentNetworkRedials"` (its `vpn` path, `utun4` beside `en0`, at :11): another router, a VPN coming
/// up and a network coming back redial; the same network does not.
#[test]
fn a_different_network_redials() {
    let (wifi, other, vpn) = paths();
    let none = NetworkPath::from_adapters(&Adapters { adapters: vec![adapter("lo0", &["127.0.0.1", "::1"])], unreadable: 0 });
    assert!(wifi.satisfied && vpn.satisfied && !none.satisfied, "loopback alone is no network");
    assert_eq!(vpn.interfaces, ["en0", "utun4"], "the tunnel is part of the path, loopback is not");
    let action = |old: &NetworkPath, next: &NetworkPath| policy::action(Some(old), Some(next), false, false);
    assert_eq!(action(&wifi, &other), LinkAction::Reconnect);
    assert_eq!(action(&wifi, &vpn), LinkAction::Reconnect);
    assert_eq!(action(&none, &wifi), LinkAction::Reconnect);
    assert_eq!(action(&wifi, &paths().0), LinkAction::None);
}

/// `RelayLinkTests.swift:39 "anOnDemandVPNCountsAsANetwork"` (`requiresConnection` with `utun4` alone, at :41): a path carried by a
/// tunnel alone is a network, so the link dials (which is what brings such a VPN up) instead of going offline.
#[test]
fn an_on_demand_vpn_counts_as_a_network() {
    let tunnel = NetworkPath::from_adapters(&Adapters { adapters: vec![adapter("utun4", &["fd7a:115c:a1e0::5"])], unreadable: 0 });
    assert!(tunnel.satisfied && tunnel.interfaces == ["utun4"]);
    let offline = NetworkPath::from_adapters(&Adapters { adapters: vec![adapter("utun4", &["fe80::5"])], unreadable: 0 });
    assert!(!offline.satisfied, "a tunnel with a link-local address only carries nothing");
    assert_eq!(policy::action(Some(&offline), Some(&tunnel), false, false), LinkAction::Reconnect, "the tunnel's path dials");
    assert_eq!(policy::action(Some(&tunnel), Some(&offline), false, false), LinkAction::Offline);
}

/// This Mac's path read twice through its network port is the same, and leaves loopback and link-local addresses out.
#[cfg(target_os = "macos")]
#[test]
fn this_macs_path_reads_the_same_twice_without_loopback() {
    use mighty_platform::Network as _;
    let network = mighty_platform_macos::net::MacNetwork;
    let first = NetworkPath::from_adapters(&network.adapters().unwrap());
    let second = NetworkPath::from_adapters(&network.adapters().unwrap());
    assert_eq!(first, second);
    assert!(!first.interfaces.iter().any(|name| name == "lo0"), "{first:?}");
    assert!(first.gateways.iter().all(|g| g.parse::<IpAddr>().is_ok_and(|ip| !ip.is_loopback())), "{first:?}");
}
