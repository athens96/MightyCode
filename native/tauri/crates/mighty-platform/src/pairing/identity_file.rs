//! `<profile>\mobile-remote\mobile-identity.json` as C# reads and writes it (R\MobileIdentity.cs:46-60, 150-166; plan §1.2, D14): plain
//! JSON through `Wire.Json` — `{version:1, hostId, hostToken, pairingKey, secretKeyB64, devices:[{id, name, tokenHash, firstSeen,
//! lastSeen, push?:{handle, kinds, updatedAt}}]}` — in an owner-only folder (no DPAPI). Shared and pure, so it is tested on both OSes;
//! the Windows crate does the file's IO and the folder's ACL (mighty-platform-windows `pairing.rs`). Owner: W8-A.
//!
//! At rest (W8 Step 0): neither direction is written yet; both answer `Unsupported`.

use crate::{PairingIdentity, PlatformError, Result};

/// The folder under the profile (C# `MobileIdentity(Path.Combine(dataDir, "mobile-remote"))`).
pub const FOLDER: &str = "mobile-remote";
/// The file in it.
pub const FILE: &str = "mobile-identity.json";
/// A larger file is refused unread (`MobileIdentity.cs:49`).
pub const MAXIMUM_BYTES: u64 = 128 * 1024;
/// At most this many phones (`state.Devices.Count > 32`).
pub const MAXIMUM_DEVICES: usize = 32;

/// The file's text read as C#'s constructor reads it: `Err` for any damage it refuses (the port's contract: the caller decides),
/// a push registration the row cannot use dropped with the phone kept.
pub fn decode(text: &str) -> Result<PairingIdentity> {
    let _ = text;
    Err(PlatformError::unsupported("identity_file", "decode"))
}

/// The text C#'s `Save` writes for `identity` (`Wire.Json`: camelCase, nulls left out, C#'s member order and escaping).
pub fn encode(identity: &PairingIdentity) -> Result<String> {
    let _ = identity;
    Err(PlatformError::unsupported("identity_file", "encode"))
}
