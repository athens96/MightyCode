//! M-B: the phone's pairing identity in Swift's four files under `<data>/mobile-remote/` (`mighty_platform::pairing::four_files`,
//! shared pure code chosen by `PlatformProfile::pairing_layout`). Swift `MobileRemoteService.swift:308-466`,
//! `MobileDeviceRegistry.swift`, `RelayChannel.swift:86-104`; contract `mighty_platform::pairing` (`PairingStore`). Swift-written
//! fixtures read here, and what this writes read back by Swift (`tools/parity-mac pairing`).

use std::sync::Arc;

use mighty_platform::unsupported::Unsupported;
use mighty_platform::{PairingStore, Paths};

/// The Mac's `PairingStore` over the profile's data folder (Unsupported until M-B).
pub fn pairing_store(_paths: &dyn Paths) -> Arc<dyn PairingStore> {
    Arc::new(Unsupported)
}
