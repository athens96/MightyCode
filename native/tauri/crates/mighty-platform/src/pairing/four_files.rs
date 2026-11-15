//! The Mac's pairing layout ([`crate::PairingLayout::FourFiles`]): the phone's pairing identity in Swift's four files under
//! `<data>/mobile-remote/` (folder 0700), read and written in Swift's formats so a phone paired with 0.4.x keeps working after the
//! update (Swift `MobileRemoteService.swift:308-466`, `MobileDeviceRegistry.swift:22-35,402-475`, `RelayChannel.swift:86-104`).
//! Shared, pure code, no `cfg`: chosen by the profile and tested on both OSes. Mac stage Step 0 names the files; the formats, the
//! load and the save are M-B's (with `tools/parity-mac pairing` as Swift's own oracle both ways).

/// The folder under the data folder.
pub const FOLDER: &str = "mobile-remote";
/// The pairing key: 32 bytes as base64url, mode 0600.
pub const PAIRING_KEY: &str = "mobile-remote.key";
/// The relay host token: plain hex text (`^[A-Fa-f0-9]{32,64}$`) despite the name.
pub const HOST_TOKEN: &str = "relay-host-token.json";
/// The relay key pair: `{"v":1,"publicKeyB64","secretKeyB64"}` with sorted keys.
pub const KEY_PAIR: &str = "relay-keypair.json";
/// The paired phones: an array of `MobileDevice{id, name, tokenHash?, firstSeen, lastSeen, push?}`, ISO 8601 with fractional seconds.
pub const DEVICES: &str = "devices.json";
