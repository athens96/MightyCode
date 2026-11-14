//! The relay's cipher (C# R\RelayCrypto.cs; plan §1.1, read by `native/contracts/relay-vectors.json`, which the phone's
//! `relay-interop.test.ts` and Swift's `RelayInteropVectorTests` read too): `shared = X25519(hostSecret, clientPublic)` (all-zero
//! refused), `key = HKDF-SHA256(ikm=shared, salt=clientNonce‖serverNonce, info="mightyclaude-relay-v1", L=32)`; a frame is
//! `nonce(12) = [dir, 0,0,0, counter u64 BE]` ‖ ChaCha20-Poly1305 ciphertext ‖ tag(16), no AAD; the receiver checks size (28 bytes to
//! 1 MiB), direction, the three zero bytes, then a counter above the last accepted, and moves it only after a decrypt. Secrets are
//! wiped after use. Owner: W8-A.
//!
//! At rest (W8 Step 0): the constants only.

/// HKDF's `info`.
pub const INFO: &[u8] = b"mightyclaude-relay-v1";
/// The direction byte of a frame the host sends.
pub const HOST_TO_PHONE: u8 = 2;
/// The direction byte of a frame the phone sends.
pub const PHONE_TO_HOST: u8 = 1;
/// The nonce and the tag around a frame's ciphertext.
pub const OVERHEAD: usize = 28;
/// The largest frame either side takes (`MobileDirect.MaximumFrameBytes`).
pub const MAXIMUM_FRAME: usize = 1024 * 1024;
/// `MobileRelayHost.MaximumPlaintext`.
pub const MAXIMUM_PLAINTEXT: usize = MAXIMUM_FRAME - OVERHEAD;
