//! The push envelope (C# `MobilePushSeal`, R\MobilePush.cs; plan §1.3): `key = HKDF-SHA256(ikm = the device's hex-decoded token
//! hash, salt = UTF-8 clientId, info = "mighty-push-v1", L=32)` (none unless the hash is 64 hex digits); `envelope =
//! base64url-no-pad(0x01 ‖ nonce(12) ‖ ChaCha20-Poly1305 ct ‖ tag)` with `AAD = "mighty-push-v1:" + handle`; plaintext at most 2048
//! bytes, envelope at most 3072 characters. Owner: W8-A.

use base64::Engine;
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce};
use hkdf::Hkdf;
use sha2::Sha256;
use zeroize::Zeroizing;

pub const INFO: &[u8] = b"mighty-push-v1";
pub const AAD_PREFIX: &str = "mighty-push-v1:";
pub const VERSION: u8 = 1;
pub const MAXIMUM_PLAINTEXT: usize = 2048;
pub const MAXIMUM_ENVELOPE: usize = 3072;

/// `MobilePushSeal.Key`: the key, or None when the hash is not 64 ASCII hex digits (either case).
pub fn key(token_hash: &str, client_id: &str) -> Option<Zeroizing<[u8; 32]>> {
    if token_hash.len() != 64 || !token_hash.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let ikm: Vec<u8> = (0..32).map(|i| u8::from_str_radix(&token_hash[i * 2..i * 2 + 2], 16)).collect::<Result<_, _>>().ok()?;
    let ikm = Zeroizing::new(ikm);
    let mut key = Zeroizing::new([0u8; 32]);
    Hkdf::<Sha256>::new(Some(client_id.as_bytes()), &ikm).expand(INFO, key.as_mut()).ok()?;
    Some(key)
}

fn aad(handle: &str) -> Vec<u8> {
    format!("{AAD_PREFIX}{handle}").into_bytes()
}

/// Why a push was not sealed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SealError {
    /// "Push message too long." (C#'s `ArgumentException`)
    TooLong,
}

/// `MobilePushSeal.Seal`: the envelope for `plaintext` under `key` and `handle` (a random nonce unless one is given, as the checks do).
pub fn seal(plaintext: &[u8], key: &[u8; 32], handle: &str, nonce: Option<[u8; 12]>) -> Result<String, SealError> {
    if plaintext.len() > MAXIMUM_PLAINTEXT {
        return Err(SealError::TooLong);
    }
    let nonce = nonce.unwrap_or_else(|| {
        let mut fresh = [0u8; 12];
        getrandom::fill(&mut fresh).expect("the OS random source");
        fresh
    });
    let sealed = ChaCha20Poly1305::new(Key::from_slice(key))
        .encrypt(Nonce::from_slice(&nonce), Payload { msg: plaintext, aad: &aad(handle) })
        .map_err(|_| SealError::TooLong)?;
    let mut envelope = Vec::with_capacity(13 + sealed.len());
    envelope.push(VERSION);
    envelope.extend_from_slice(&nonce);
    envelope.extend_from_slice(&sealed);
    Ok(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(envelope))
}

/// `MobilePushSeal.Open`: the plaintext, or None for anything that is not this handle's envelope under this key.
pub fn open(envelope: &str, key: &[u8; 32], handle: &str) -> Option<Vec<u8>> {
    if envelope.len() > MAXIMUM_ENVELOPE || !envelope.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-') {
        return None;
    }
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(envelope)
        .or_else(|_| {
            base64::engine::GeneralPurpose::new(
                &base64::alphabet::URL_SAFE,
                base64::engine::general_purpose::NO_PAD.with_decode_allow_trailing_bits(true),
            )
            .decode(envelope)
        })
        .ok()?;
    if bytes.len() < 1 + 12 + 16 || bytes[0] != VERSION {
        return None;
    }
    ChaCha20Poly1305::new(Key::from_slice(key))
        .decrypt(Nonce::from_slice(&bytes[1..13]), Payload { msg: &bytes[13..], aad: &aad(handle) })
        .ok()
}
