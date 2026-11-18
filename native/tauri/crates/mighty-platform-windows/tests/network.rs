//! The network port on Windows (net.rs, W8-C): what the fake's contract states (mighty-platform's `Network`), by the real calls — on
//! loopback port 0 only, never a real interface or a wildcard.
#![cfg(windows)]

use std::net::{IpAddr, SocketAddr};

use mighty_platform::{Network, NetworkCallback, PlatformError};
use mighty_platform_windows::WindowsNetwork;

/// An exclusive listener holds its address and port alone: a second bind of the same pair is `AddrInUse` (or refused), and the port is
/// free again once the first is dropped. IPv6 loopback binds IPv6 only.
#[test]
fn an_exclusive_listener_holds_its_port_alone() {
    let network = WindowsNetwork;
    let loopback = SocketAddr::new(IpAddr::from([127, 0, 0, 1]), 0);
    let first = network.bind_exclusive(loopback, 36).expect("loopback port 0");
    let taken = first.local_addr().unwrap();
    assert!(taken.port() > 0 && taken.ip().is_loopback());
    match network.bind_exclusive(taken, 36) {
        Err(PlatformError::Io(error)) => assert!(
            matches!(error.kind(), std::io::ErrorKind::AddrInUse | std::io::ErrorKind::PermissionDenied),
            "a second bind is refused as in use: {error}"
        ),
        other => panic!("a second bind of an exclusive port: {other:?}"),
    }
    // Even a plain socket asking for the same address and port is refused.
    assert!(std::net::TcpListener::bind(taken).is_err(), "no other socket shares an exclusive port");
    drop(first);
    let again = network.bind_exclusive(taken, 36).expect("the port is free again");
    drop(again);
    if let Ok(v6) = network.bind_exclusive(SocketAddr::new(IpAddr::from([0u16, 0, 0, 0, 0, 0, 0, 1]), 0), 36) {
        assert!(v6.local_addr().unwrap().is_ipv6());
    }
}

/// The adapters read: the up ones with their addresses (loopback's at least, when the OS lists it), never a failure on a working PC;
/// address-change and resume subscriptions register and end without a call back.
#[test]
fn the_adapters_read_and_the_subscriptions_end() {
    let network = WindowsNetwork;
    let adapters = network.adapters().expect("this PC's adapters");
    assert_eq!(adapters.unreadable, 0);
    assert!(adapters.adapters.iter().all(|adapter| !adapter.name.is_empty()), "every adapter has its name");
    let change = network.on_change(NetworkCallback::new(|| {})).expect("address changes");
    let resume = network.on_resume(NetworkCallback::new(|| {})).expect("resumes");
    drop(change);
    drop(resume);
}
