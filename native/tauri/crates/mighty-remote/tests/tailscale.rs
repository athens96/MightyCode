//! C#'s `TailscaleAddressesVerification` (Tests\TailscaleAddressesVerification.cs; plan W8-C, L485-L488), ported by name.
//! `native/contracts/fixtures/tailscale-addresses.json` is the address rule the Mac checks too: Windows reads each case's `win32`
//! answer and the Mac's rule (`needs_utun`) its `darwin` one.

use std::net::IpAddr;
use std::path::Path;

use mighty_platform::fake::FakeNetwork;
use mighty_platform::{Adapter, Adapters, Network};
use mighty_remote::tailscale::{self, canonical_text, contains, in_tailscale_ipv6_range, select, select_text};
use serde_json::Value;

fn fixture() -> Value {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../contracts/fixtures/tailscale-addresses.json");
    serde_json::from_str(&std::fs::read_to_string(path).expect("the shared fixture")).expect("the fixture is JSON")
}

fn pairs(items: &[(&str, &str)]) -> Vec<(String, String)> {
    items.iter().map(|(name, address)| (name.to_string(), address.to_string())).collect()
}

fn adapter(name: &str, addresses: &[&str]) -> Adapter {
    Adapter { name: name.into(), addresses: addresses.iter().map(|a| a.parse::<IpAddr>().unwrap()).collect() }
}

/// "tailscale addresses: the shared fixture's cases" (L485).
#[test]
fn tailscale_addresses_the_shared_fixtures_cases() {
    let root = fixture();
    let strings = |value: &Value| value.as_array().unwrap().iter().map(|v| v.as_str().unwrap().to_string()).collect::<Vec<_>>();
    for address in strings(&root["contains"]["accepted"]) {
        assert!(contains(&address), "in Tailscale's ranges: {address}");
    }
    for address in strings(&root["contains"]["rejected"]) {
        assert!(!contains(&address), "outside Tailscale's ranges: {address}");
    }
    let cases = root["cases"].as_array().unwrap();
    assert!(cases.len() >= 12, "the shared fixture was not loaded");
    for case in cases {
        let read: Vec<(String, String)> = case["interfaces"]
            .as_array()
            .unwrap()
            .iter()
            .flat_map(|adapter| {
                let name = adapter["name"].as_str().unwrap().to_string();
                strings(&adapter["addresses"]).into_iter().map(move |address| (name.clone(), address))
            })
            .collect();
        let name = case["name"].as_str().unwrap();
        assert_eq!(select_text(&read, false), strings(&case["expected"]["win32"]), "{name} (win32)");
        assert_eq!(select_text(&read, true), strings(&case["expected"]["darwin"]), "{name} (darwin)");
    }
}

/// "tailscale addresses: a mapped address counts as its IPv4" (L486): an IPv4-mapped address in Tailscale's CGNAT range is taken as
/// its IPv4 and spelled the way the OS writes it (still mapped, so listed with the IPv6 addresses), as the Mac does.
#[test]
fn tailscale_addresses_a_mapped_address_counts_as_its_ipv4() {
    assert!(contains("::ffff:100.100.1.1") && contains("[::FFFF:100.100.1.1]") && contains("::ffff:6464:101"), "a mapped CGNAT address");
    assert!(
        !contains("::ffff:100.128.0.1") && !contains("::ffff:192.168.0.5") && !contains("::ffff:100.100.1.1%7"),
        "a mapped address outside the range, or scoped, is not"
    );
    assert!(!in_tailscale_ipv6_range("::ffff:100.100.1.1"), "a mapped address does not make an adapter Tailscale's");
    let on_tunnel =
        select_text(&pairs(&[("utun4", "fd7a:115c:a1e0::5"), ("utun4", "::ffff:100.100.1.1"), ("utun4", "100.101.1.2")]), false);
    assert_eq!(on_tunnel, vec!["100.101.1.2", "::ffff:100.100.1.1", "fd7a:115c:a1e0::5"], "mapped address on a tunnel");
    assert!(select_text(&pairs(&[("utun4", "::ffff:100.100.1.1")]), false).is_empty(), "a mapped address alone is not Tailscale's");
    assert!(tailscale::is_tailscale("::ffff:100.100.1.1".parse().unwrap()), "the parsed form agrees");
    assert_eq!(tailscale::peer("::ffff:100.101.102.103".parse().unwrap()), "100.101.102.103".parse::<IpAddr>().unwrap(), "a peer's IPv4");
}

/// "tailscale addresses: detection offers only Tailscale addresses" (L487): whatever this machine's adapters are, only Tailscale
/// addresses come back, each on an adapter that carries an fd7a one; and one spelling per address.
#[test]
fn tailscale_addresses_detection_offers_only_tailscale_addresses() {
    // This machine's adapters through its own network port: Windows' (any adapter name), the Mac's (M-C: `getifaddrs`, and the
    // Mac's rule, a `utun` only).
    #[cfg(any(windows, target_os = "macos"))]
    {
        #[cfg(windows)]
        let (read, needs_utun) = (mighty_platform_windows::WindowsNetwork.adapters().expect("this PC's adapters"), false);
        #[cfg(target_os = "macos")]
        let (read, needs_utun) = (mighty_platform_macos::net::MacNetwork.adapters().expect("this Mac's adapters"), true);
        let flat: Vec<(String, String)> =
            read.adapters.iter().flat_map(|a| a.addresses.iter().map(|ip| (a.name.clone(), ip.to_string()))).collect();
        for (name, address) in tailscale::owned(&flat, needs_utun) {
            assert!(
                contains(&address)
                    && flat.iter().any(|(n, a)| *n == name && in_tailscale_ipv6_range(&canonical_text(a)))
                    && (!needs_utun || name.starts_with("utun")),
                "detection offered {address} on {name}"
            );
        }
        assert!(select(&read, needs_utun).addresses.iter().all(|ip| tailscale::is_tailscale(*ip)), "only Tailscale addresses come back");
    }
    assert!(
        canonical_text("FD7A:115C:A1E0:0::5%12") == "fd7a:115c:a1e0::5"
            && canonical_text("[fd7a:115c:a1e0::5]") == "fd7a:115c:a1e0::5"
            && canonical_text("100.101.1.2") == "100.101.1.2",
        "one spelling per address"
    );
    assert_eq!(tailscale::canonical("FD7A:115C:A1E0:0::5".parse().unwrap()), "fd7a:115c:a1e0::5");
}

/// "tailscale addresses: one unreadable adapter is skipped, none readable is a fault" (L488), over the network port's read: the port
/// keeps the readable adapters and counts the others; only when none could be read is that a fault, which the direct path shows as its
/// "adapters unreadable" words.
#[test]
fn tailscale_addresses_one_unreadable_adapter_is_skipped_none_readable_is_a_fault() {
    let read = Adapters {
        adapters: vec![adapter("Wi-Fi", &["192.168.0.5"]), adapter("Tailscale", &["100.101.102.103", "fd7a:115c:a1e0::1"])],
        unreadable: 1,
    };
    let selection = select(&read, false);
    assert!(
        !selection.unreadable
            && selection.addresses == vec!["100.101.102.103".parse::<IpAddr>().unwrap(), "fd7a:115c:a1e0::1".parse().unwrap()],
        "an unreadable adapter is skipped and the others are kept"
    );
    assert_eq!(select(&Adapters::default(), false), Default::default(), "no adapter is no fault");
    assert!(!select(&Adapters { adapters: vec![adapter("Wi-Fi", &[])], unreadable: 0 }, false).unreadable, "nor one with no address");
    assert!(select(&Adapters { adapters: vec![], unreadable: 2 }, false).unreadable, "every adapter unreadable is a fault");
    // The fake port: what a test sets is what is read; a refused read is the port's error, never a list.
    let network = FakeNetwork::default();
    network.set_adapters(read.clone());
    assert_eq!(network.adapters().unwrap(), read);
    network.fail(true);
    assert!(network.adapters().is_err(), "the whole read refused");
}
