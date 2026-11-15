//! Ed25519 as C#'s `Ed25519Verify.cs` decides it (plan §1.2, D6). Owner: W9-A.
//!
//! `ed25519-dalek`'s `VerifyingKey::verify` (cofactorless, the recomputed R compared by encoding), behind a canonical-encoding check of
//! the key (curve25519-dalek takes a y ≥ p, and an x = 0 with the sign bit set, both of which C# refuses). Never the strict check: it
//! refuses small-order points C# accepts (a grep check keeps it out). The verdicts equal C#'s on the corpus
//! `native/contracts/fixtures/update-ed25519.json` (`tools/parity/Update.cs ed25519`): RFC 8032 §7.1, S ≥ L, non-canonical R and A,
//! x = 0 with the sign bit, small-order and mixed-order points, wrong lengths.
//!
//! Why the rest agrees with C# without more code: S ≥ L is refused by dalek's canonical scalar check (the crate's
//! `legacy_compatibility` feature, which would loosen it, is off); R is never decoded by dalek but compared by its canonical
//! encoding, so an R C# cannot decode (off the curve, y ≥ p, x = 0 with the sign bit) never equals it; for an R C# decodes, equal
//! points have equal canonical encodings; k is SHA-512(R ‖ A ‖ M) over the key's bytes as given in both.

use ed25519_dalek::{Signature, Verifier, VerifyingKey};

/// A raw public key's length (C# `Ed25519Verify.PublicKeyBytes`).
pub const PUBLIC_KEY_LENGTH: usize = 32;

/// A signature's length.
pub const SIGNATURE_LENGTH: usize = 64;

/// p = 2^255 − 19, little-endian.
const P: [u8; 32] = {
    let mut p = [0xff; 32];
    p[0] = 0xed;
    p[31] = 0x7f;
    p
};

/// C#'s `TryDecode` refusals that curve25519-dalek's decompression lets through: a y that is not below p, and the sign bit set on
/// a point whose x is 0 (y = 1 or y = p − 1). Points off the curve are refused by both.
fn canonical(encoded: &[u8; 32]) -> bool {
    let sign = encoded[31] & 0x80 != 0;
    let mut y = *encoded;
    y[31] &= 0x7f;
    // y < p, compared from the most significant byte.
    let below_p = y.iter().rev().zip(P.iter().rev()).find(|(a, b)| a != b).is_some_and(|(a, b)| a < b);
    if !below_p {
        return false;
    }
    let mut one = [0u8; 32];
    one[0] = 1;
    let mut p_minus_one = P;
    p_minus_one[0] -= 1;
    !(sign && (y == one || y == p_minus_one))
}

/// C# `Ed25519Verify.Verify(message, signature, publicKey)`: true only for a valid signature; every malformed input is false.
pub fn verify(public_key: &[u8], message: &[u8], signature: &[u8]) -> bool {
    let (Ok(key), Ok(signature)) = (<[u8; PUBLIC_KEY_LENGTH]>::try_from(public_key), <[u8; SIGNATURE_LENGTH]>::try_from(signature)) else {
        return false;
    };
    if !canonical(&key) {
        return false;
    }
    let Ok(key) = VerifyingKey::from_bytes(&key) else { return false };
    key.verify(message, &Signature::from_bytes(&signature)).is_ok()
}
