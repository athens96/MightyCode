//! The push envelope (C# `MobilePushSeal`, R\MobilePush.cs; plan §1.3): `key = HKDF-SHA256(ikm = the device's hex-decoded token
//! hash, salt = UTF-8 clientId, info = "mighty-push-v1", L=32)` (none unless the hash is 64 hex digits); `envelope =
//! base64url-no-pad(0x01 ‖ nonce(12) ‖ ChaCha20-Poly1305 ct ‖ tag)` with `AAD = "mighty-push-v1:" + handle`; plaintext at most 2048
//! bytes, envelope at most 3072 characters. Owner: W8-A.
//!
//! At rest (W8 Step 0): the constants only.

pub const INFO: &[u8] = b"mighty-push-v1";
pub const AAD_PREFIX: &str = "mighty-push-v1:";
pub const MAXIMUM_PLAINTEXT: usize = 2048;
pub const MAXIMUM_ENVELOPE: usize = 3072;
