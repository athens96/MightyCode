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
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;
use x25519_dalek::{PublicKey, StaticSecret};
use zeroize::{Zeroize, Zeroizing};

/// HKDF's `info`.
pub const INFO: &[u8] = b"mightyclaude-relay-v1";
/// Forward-secret (`fs: 1`) transcript domain prefix (18 ASCII bytes).
pub const FS_TRANSCRIPT_PREFIX: &[u8] = b"mighty-relay-fs-v1";
/// Forward-secret (`fs: 1`) HKDF `info` prefix (24 ASCII bytes, followed by the 32-byte transcript SHA-256).
pub const INFO_FS_PREFIX: &[u8] = b"mightyclaude-relay-fs-v1";
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
    strict_sequence: bool,
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

#[allow(clippy::too_many_arguments, reason = "exact fs:1 transcript inputs")]
fn derive_fs_cipher(
    shared_static: &[u8; 32],
    shared_eph: &[u8; 32],
    client_pub: &[u8; 32],
    host_static_pub: &[u8; 32],
    host_eph_pub: &[u8; 32],
    client_nonce: &[u8; 16],
    server_nonce: &[u8; 16],
    host: bool,
) -> Result<RelayCipher, CipherError> {
    let zero_static = bool::from(shared_static.ct_eq(&[0u8; 32]));
    let zero_eph = bool::from(shared_eph.ct_eq(&[0u8; 32]));
    if zero_static || zero_eph {
        return Err(CipherError::Agreement);
    }
    let mut ikm = Zeroizing::new([0u8; 64]);
    ikm[..32].copy_from_slice(shared_static);
    ikm[32..].copy_from_slice(shared_eph);

    let mut salt = [0u8; 32];
    salt[..16].copy_from_slice(client_nonce);
    salt[16..].copy_from_slice(server_nonce);

    let mut hasher = Sha256::new();
    hasher.update(FS_TRANSCRIPT_PREFIX);
    hasher.update(client_pub);
    hasher.update(host_static_pub);
    hasher.update(host_eph_pub);
    hasher.update(client_nonce);
    hasher.update(server_nonce);
    let transcript = hasher.finalize();

    let mut info = [0u8; 56];
    info[..24].copy_from_slice(INFO_FS_PREFIX);
    info[24..].copy_from_slice(&transcript);

    let mut key = Zeroizing::new([0u8; 32]);
    Hkdf::<Sha256>::new(Some(&salt), ikm.as_ref()).expand(&info, key.as_mut()).map_err(|_| CipherError::Lengths)?;
    ikm.zeroize();

    let (send_direction, receive_direction) = if host { (HOST_TO_PHONE, PHONE_TO_HOST) } else { (PHONE_TO_HOST, HOST_TO_PHONE) };
    Ok(RelayCipher {
        send_direction,
        receive_direction,
        strict_sequence: true,
        state: Mutex::new(Counters { key, sent: 0, received: -1, disposed: false }),
    })
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
        Ok(RelayCipher {
            send_direction,
            receive_direction,
            strict_sequence: false,
            state: Mutex::new(Counters { key, sent: 0, received: -1, disposed: false }),
        })
    }

    /// Forward-secret (`fs: 1`) host cipher: static + ephemeral X25519 agreement bound to the handshake transcript, with strict
    /// gap-free receive counters.
    pub fn new_fs_host(
        host_static_secret: &[u8],
        host_eph_secret: &[u8],
        client_pub: &[u8],
        host_static_pub: &[u8],
        host_eph_pub: &[u8],
        client_nonce: &[u8],
        server_nonce: &[u8],
    ) -> Result<RelayCipher, CipherError> {
        if host_static_secret.len() != 32 || host_eph_secret.len() != 32 {
            return Err(CipherError::Lengths);
        }
        let client_pub: [u8; 32] = client_pub.try_into().map_err(|_| CipherError::Lengths)?;
        let host_static_pub: [u8; 32] = host_static_pub.try_into().map_err(|_| CipherError::Lengths)?;
        let host_eph_pub: [u8; 32] = host_eph_pub.try_into().map_err(|_| CipherError::Lengths)?;
        let client_nonce: [u8; 16] = client_nonce.try_into().map_err(|_| CipherError::Lengths)?;
        let server_nonce: [u8; 16] = server_nonce.try_into().map_err(|_| CipherError::Lengths)?;

        let mut static_secret = Zeroizing::new([0u8; 32]);
        static_secret.copy_from_slice(host_static_secret);
        let mut eph_secret = Zeroizing::new([0u8; 32]);
        eph_secret.copy_from_slice(host_eph_secret);

        let peer = PublicKey::from(client_pub);
        let mut static_scalar = StaticSecret::from(*static_secret);
        static_secret.zeroize();
        let shared_static = static_scalar.diffie_hellman(&peer);
        static_scalar.zeroize();

        let mut eph_scalar = StaticSecret::from(*eph_secret);
        eph_secret.zeroize();
        let shared_eph = eph_scalar.diffie_hellman(&peer);
        eph_scalar.zeroize();

        derive_fs_cipher(
            shared_static.as_bytes(),
            shared_eph.as_bytes(),
            &client_pub,
            &host_static_pub,
            &host_eph_pub,
            &client_nonce,
            &server_nonce,
            true,
        )
    }

    /// Forward-secret (`fs: 1`) client cipher: static + ephemeral X25519 agreement bound to the handshake transcript, with strict
    /// gap-free receive counters.
    pub fn new_fs_client(
        client_eph_secret: &[u8],
        client_pub: &[u8],
        host_static_pub: &[u8],
        host_eph_pub: &[u8],
        client_nonce: &[u8],
        server_nonce: &[u8],
    ) -> Result<RelayCipher, CipherError> {
        if client_eph_secret.len() != 32 {
            return Err(CipherError::Lengths);
        }
        let client_pub: [u8; 32] = client_pub.try_into().map_err(|_| CipherError::Lengths)?;
        let host_static_pub: [u8; 32] = host_static_pub.try_into().map_err(|_| CipherError::Lengths)?;
        let host_eph_pub: [u8; 32] = host_eph_pub.try_into().map_err(|_| CipherError::Lengths)?;
        let client_nonce: [u8; 16] = client_nonce.try_into().map_err(|_| CipherError::Lengths)?;
        let server_nonce: [u8; 16] = server_nonce.try_into().map_err(|_| CipherError::Lengths)?;

        let mut client_secret = Zeroizing::new([0u8; 32]);
        client_secret.copy_from_slice(client_eph_secret);
        let mut scalar = StaticSecret::from(*client_secret);
        client_secret.zeroize();
        let shared_static = scalar.diffie_hellman(&PublicKey::from(host_static_pub));
        let shared_eph = scalar.diffie_hellman(&PublicKey::from(host_eph_pub));
        scalar.zeroize();

        derive_fs_cipher(
            shared_static.as_bytes(),
            shared_eph.as_bytes(),
            &client_pub,
            &host_static_pub,
            &host_eph_pub,
            &client_nonce,
            &server_nonce,
            false,
        )
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
        if self.strict_sequence {
            let Some(expected) = state.received.checked_add(1).and_then(|n| u64::try_from(n).ok()) else {
                return Err(CipherError::Replayed);
            };
            if counter > i64::MAX as u64 || counter != expected {
                return Err(CipherError::Replayed);
            }
        } else if counter > i64::MAX as u64 || counter as i64 <= state.received {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }

    #[test]
    fn fs_v1_deterministic_vector_and_strict_sequence() {
        let host_static_secret = [0x11u8; 32];
        let host_eph_secret = [0x22u8; 32];
        let client_eph_secret = [0x33u8; 32];
        let client_nonce = [0x44u8; 16];
        let server_nonce = [0x55u8; 16];

        let host_static_pub = public_key(&host_static_secret);
        let host_eph_pub = public_key(&host_eph_secret);
        let client_pub = public_key(&client_eph_secret);

        let host = RelayCipher::new_fs_host(
            &host_static_secret,
            &host_eph_secret,
            &client_pub,
            &host_static_pub,
            &host_eph_pub,
            &client_nonce,
            &server_nonce,
        )
        .unwrap();
        let client =
            RelayCipher::new_fs_client(&client_eph_secret, &client_pub, &host_static_pub, &host_eph_pub, &client_nonce, &server_nonce)
                .unwrap();

        let host_key_hex = hex(host.state().key.as_ref());
        let client_key_hex = hex(client.state().key.as_ref());
        assert_eq!(host_key_hex, client_key_hex, "host and client derive identical 32B FS key");
        assert_eq!(host_key_hex, "49e1f5131039cdaaf2d57605b329b5c034a2aca27343150760e556da99ad9690");

        let c_frame0 = client.seal(b"hello-fs").unwrap();
        assert_eq!(hex(&c_frame0), "010000000000000000000000ebf73608cded1ac015d9aec301d81ebefe622c7a9918de8e");
        assert_eq!(host.open(&c_frame0).unwrap(), b"hello-fs");
        assert_eq!(host.open(&c_frame0), Err(CipherError::Replayed));

        let h_frame0 = host.seal(b"hello-fs").unwrap();
        assert_eq!(hex(&h_frame0), "02000000000000000000000027f63a7c4d10f7cc6e60bc2a7f275af869c7885b7e7c5473");
        assert_eq!(client.open(&h_frame0).unwrap(), b"hello-fs");

        // Strict sequence: skipping counter 1 (sending counter 2 next) fails on fs:1, while a forged tag does not advance received.
        let mut c_frame1 = client.seal(b"frame-1").unwrap();
        let c_frame2 = client.seal(b"frame-2").unwrap();
        assert_eq!(host.open(&c_frame2), Err(CipherError::Replayed), "gap from 0 -> 2 rejected in strict mode");
        *c_frame1.last_mut().unwrap() ^= 1;
        assert_eq!(host.open(&c_frame1), Err(CipherError::Forged), "forged tag rejected");
        *c_frame1.last_mut().unwrap() ^= 1;
        assert_eq!(host.open(&c_frame1).unwrap(), b"frame-1", "counter 1 still accepted after forged frame");
        assert_eq!(host.open(&c_frame2).unwrap(), b"frame-2", "counter 2 accepted in order");

        // Compare with v1: gap 0 -> 2 succeeds in v1.
        let v1_host = RelayCipher::new(&host_static_secret, &client_pub, &client_nonce, &server_nonce, true).unwrap();
        let v1_client = RelayCipher::new(&client_eph_secret, &host_static_pub, &client_nonce, &server_nonce, false).unwrap();
        let v1_f0 = v1_client.seal(b"v1-0").unwrap();
        let _v1_f1 = v1_client.seal(b"v1-1").unwrap();
        let v1_f2 = v1_client.seal(b"v1-2").unwrap();
        assert_eq!(v1_host.open(&v1_f0).unwrap(), b"v1-0");
        assert_eq!(v1_host.open(&v1_f2).unwrap(), b"v1-2", "v1 remains gap-tolerant");

        // Low-order peer public keys and invalid input lengths rejected.
        assert_eq!(
            RelayCipher::new_fs_host(
                &host_static_secret,
                &host_eph_secret,
                &[0u8; 32],
                &host_static_pub,
                &host_eph_pub,
                &client_nonce,
                &server_nonce
            )
            .err(),
            Some(CipherError::Agreement)
        );
        assert_eq!(
            RelayCipher::new_fs_client(&client_eph_secret, &client_pub, &[0u8; 32], &host_eph_pub, &client_nonce, &server_nonce).err(),
            Some(CipherError::Agreement)
        );
        assert_eq!(
            RelayCipher::new_fs_client(&client_eph_secret, &client_pub, &host_static_pub, &[0u8; 32], &client_nonce, &server_nonce).err(),
            Some(CipherError::Agreement)
        );
        assert_eq!(
            RelayCipher::new_fs_client(
                &client_eph_secret[..31],
                &client_pub,
                &host_static_pub,
                &host_eph_pub,
                &client_nonce,
                &server_nonce
            )
            .err(),
            Some(CipherError::Lengths)
        );
        assert_eq!(
            RelayCipher::new_fs_host(
                &host_static_secret,
                &host_eph_secret,
                &client_pub,
                &host_static_pub,
                &host_eph_pub,
                &client_nonce[..15],
                &server_nonce
            )
            .err(),
            Some(CipherError::Lengths)
        );
    }
}
