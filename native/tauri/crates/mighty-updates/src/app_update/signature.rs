//! Ed25519 as C#'s `Ed25519Verify.cs` decides it (plan §1.2, D6). Owner: W9-A.
//!
//! `ed25519-dalek`'s `VerifyingKey::verify` (cofactorless, the recomputed R compared by encoding), behind a canonical-encoding check of
//! the key (curve25519-dalek takes a y ≥ p that C# refuses). Never `verify_strict`: it refuses small-order points C# accepts (a grep
//! check keeps it out). The verdicts equal C#'s on the corpus `native/contracts/fixtures/update-ed25519.json` (`tools/parity/Update.cs
//! ed25519`): RFC 8032 §7.1, S ≥ L, non-canonical R and A, x = 0 with the sign bit, small-order points, wrong lengths.

/// A raw public key's length (C# `Ed25519Verify.PublicKeyBytes`).
pub const PUBLIC_KEY_LENGTH: usize = 32;

/// A signature's length.
pub const SIGNATURE_LENGTH: usize = 64;

/// C# `Ed25519Verify.Verify(publicKey, message, signature)`. Step 0 stub (W9-A writes it): no signature verifies.
pub fn verify(_public_key: &[u8], _message: &[u8], _signature: &[u8]) -> bool {
    false
}
