//! The relay's cipher (C# R\RelayCrypto.cs; plan §1.1, read by `native/contracts/relay-vectors.json`, which the phone's
//! `relay-interop.test.ts` and Swift's `RelayInteropVectorTests` read too): `shared = X25519(hostSecret, clientPublic)` (all-zero
//! refused), `key = HKDF-SHA256(ikm=shared, salt=clientNonce‖serverNonce, info="mightyclaude-relay-v1", L=32)`; a frame is
//! `nonce(12) = [dir, 0,0,0, counter u64 BE]` ‖ ChaCha20-Poly1305 ciphertext ‖ tag(16), no AAD; the receiver checks size (28 bytes to
//! 1 MiB), direction, the three zero bytes, then a counter above the last accepted, and moves it only after a decrypt. Secrets are
//! wiped after use. Owner: W8-A.

use std::sync::Mutex;

use chacha20poly1305::aead::{Aead, KeyInit};
use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce};
use hkdf::Hkdf;
use sha2::Sha256;
use subtle::ConstantTimeEq;
use x25519_dalek::{PublicKey, StaticSecret};
use zeroize::{Zeroize, Zeroizing};

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

/// Why the cipher refused (C#'s `CryptographicException` messages; never any key material).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CipherError {
    /// "Invalid relay handshake lengths."
    Lengths,
    /// "Invalid relay agreement." (an all-zero shared secret: a low-order peer key)
    Agreement,
    /// "Relay payload exceeds limit."
    TooLarge,
    /// "Relay counter exhausted."
    Exhausted,
    /// "Invalid relay frame." (size, direction or the zero bytes)
    Frame,
    /// "Replayed relay frame." (a counter not above the last accepted)
    Replayed,
    /// The tag did not authenticate the frame.
    Forged,
    /// The key was wiped (`ObjectDisposedException`).
    Disposed,
}

impl std::fmt::Display for CipherError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            CipherError::Lengths => "Invalid relay handshake lengths.",
            CipherError::Agreement => "Invalid relay agreement.",
            CipherError::TooLarge => "Relay payload exceeds limit.",
            CipherError::Exhausted => "Relay counter exhausted.",
            CipherError::Frame => "Invalid relay frame.",
            CipherError::Replayed => "Replayed relay frame.",
            CipherError::Forged => "The relay frame did not authenticate.",
            CipherError::Disposed => "The relay cipher was disposed.",
        })
    }
}

impl std::error::Error for CipherError {}

struct Counters {
    key: Zeroizing<[u8; 32]>,
    sent: u64,
    /// The last counter accepted; -1 before the first.
    received: i64,
    disposed: bool,
}

/// C# `RelayCipher`: one session's key and both counters, behind one lock (a seal and an open never interleave a counter).
pub struct RelayCipher {
    send_direction: u8,
    receive_direction: u8,
    state: Mutex<Counters>,
}

/// The key never reaches `Debug`.
impl std::fmt::Debug for RelayCipher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RelayCipher").field("send_direction", &self.send_direction).finish_non_exhaustive()
    }
}

/// The X25519 public key of a 32-byte secret (`RelayCipher.PublicKey`; the scalar clamped as Bouncy Castle clamps it).
pub fn public_key(secret: &[u8; 32]) -> [u8; 32] {
    let secret = StaticSecret::from(*secret);
    PublicKey::from(&secret).to_bytes()
}

impl RelayCipher {
    /// `new RelayCipher(secret, peer, clientNonce, serverNonce, host)`: the session key from this side's secret, the peer's public key
    /// and both nonces; `host` picks the directions (the host sends 2 and reads 1).
    pub fn new(secret: &[u8], peer: &[u8], client_nonce: &[u8], server_nonce: &[u8], host: bool) -> Result<RelayCipher, CipherError> {
        let secret: [u8; 32] = secret.try_into().map_err(|_| CipherError::Lengths)?;
        let peer: [u8; 32] = peer.try_into().map_err(|_| CipherError::Lengths)?;
        if client_nonce.len() != 16 || server_nonce.len() != 16 {
            return Err(CipherError::Lengths);
        }
        let mut secret = Zeroizing::new(secret);
        let shared = StaticSecret::from(*secret).diffie_hellman(&PublicKey::from(peer));
        secret.zeroize();
        if bool::from(shared.as_bytes().ct_eq(&[0u8; 32])) {
            return Err(CipherError::Agreement);
        }
        let mut salt = [0u8; 32];
        salt[..16].copy_from_slice(client_nonce);
        salt[16..].copy_from_slice(server_nonce);
        let mut key = Zeroizing::new([0u8; 32]);
        Hkdf::<Sha256>::new(Some(&salt), shared.as_bytes()).expand(INFO, key.as_mut()).map_err(|_| CipherError::Lengths)?;
        let (send_direction, receive_direction) = if host { (HOST_TO_PHONE, PHONE_TO_HOST) } else { (PHONE_TO_HOST, HOST_TO_PHONE) };
        Ok(RelayCipher { send_direction, receive_direction, state: Mutex::new(Counters { key, sent: 0, received: -1, disposed: false }) })
    }

    fn state(&self) -> std::sync::MutexGuard<'_, Counters> {
        self.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// `Seal`: one frame of `plaintext` under the next send counter.
    pub fn seal(&self, plaintext: &[u8]) -> Result<Vec<u8>, CipherError> {
        let mut state = self.state();
        if state.disposed {
            return Err(CipherError::Disposed);
        }
        if plaintext.len() > MAXIMUM_PLAINTEXT {
            return Err(CipherError::TooLarge);
        }
        if state.sent >= i64::MAX as u64 {
            return Err(CipherError::Exhausted);
        }
        let mut nonce = [0u8; 12];
        nonce[0] = self.send_direction;
        nonce[4..].copy_from_slice(&state.sent.to_be_bytes());
        state.sent += 1;
        let sealed = ChaCha20Poly1305::new(Key::from_slice(state.key.as_ref()))
            .encrypt(Nonce::from_slice(&nonce), plaintext)
            .map_err(|_| CipherError::TooLarge)?;
        let mut frame = Vec::with_capacity(12 + sealed.len());
        frame.extend_from_slice(&nonce);
        frame.extend_from_slice(&sealed);
        Ok(frame)
    }

    /// `Open`: a frame's plaintext; the receive counter moves only once it authenticated.
    pub fn open(&self, frame: &[u8]) -> Result<Vec<u8>, CipherError> {
        let mut state = self.state();
        if state.disposed {
            return Err(CipherError::Disposed);
        }
        if frame.len() > MAXIMUM_FRAME || frame.len() < OVERHEAD || frame[0] != self.receive_direction || frame[1..4] != [0, 0, 0] {
            return Err(CipherError::Frame);
        }
        let counter = u64::from_be_bytes(frame[4..12].try_into().map_err(|_| CipherError::Frame)?);
        if counter > i64::MAX as u64 || counter as i64 <= state.received {
            return Err(CipherError::Replayed);
        }
        let plain = ChaCha20Poly1305::new(Key::from_slice(state.key.as_ref()))
            .decrypt(Nonce::from_slice(&frame[..12]), &frame[12..])
            .map_err(|_| CipherError::Forged)?;
        state.received = counter as i64;
        Ok(plain)
    }

    /// `Dispose`: the key wiped; every later seal and open refused.
    pub fn dispose(&self) {
        let mut state = self.state();
        state.disposed = true;
        state.key.zeroize();
    }
}

impl Drop for RelayCipher {
    fn drop(&mut self) {
        self.dispose();
    }
}
