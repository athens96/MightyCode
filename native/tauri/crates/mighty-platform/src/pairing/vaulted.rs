//! The pairing identity's secrets — the pairing key, the relay host token and the relay's X25519 secret key — kept in this user's
//! vault ([`SecretVault`]: the Keychain on the Mac, DPAPI on Windows) rather than in the pairing files, where any process of the same
//! user (a coding agent told to by a prompt, too) could read them. The files keep what is not secret (the host id, the phones with
//! their token hashes, the relay public key) and, where a secret stood, a placeholder naming the vault item.
//!
//! One vault item holds the three secrets together (`1.<pairing key>.<host token>.<secret key, base64url>`), under an account of
//! its own (`mobile-pairing-<32 hex>`) that the files name. Secrets that change (a key rotation, the first move out of the files) go
//! into a new item first, read back, and only then do the files name it: the files' write stays the one commit point, so a failed
//! save leaves the old files naming the old item, and a rotation never pairs a new key with the old phones' list. The old item is
//! deleted after the files moved on.
//!
//! Moving out (migration): a store that reads its secrets in the files and has a vault writes them to a new item and then the files
//! without them. When the vault refuses (or reads back something else), the files keep their secrets, the app keeps working, the
//! refusal is logged once, and the vault is not tried again in this run (a locked Keychain would ask at every save); the next launch
//! tries again. A placeholder whose item cannot be read is an error (`Err`), never a fresh identity: that would unpair every phone.
//!
//! Compatibility: Swift 0.4.x and C# 0.4.x, and Rust builds before this, read a placeholder as a damaged key (Swift and the older
//! Mac build make a new key and forget the phones; C# and the older Windows build refuse the file). Going back to one of them after
//! this has run means pairing the phones again. Secrets found in the files again later (written by such a build) win over the vault
//! and are moved out again.

use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use base64::Engine;

use crate::{PairingIdentity, PlatformError, Result, SecretVault};

/// A file's text that names the vault item instead of holding a secret: `vault:<account>`.
pub const PLACEHOLDER_PREFIX: &str = "vault:";
/// The vault item's account: this, then 32 lower-case hex digits.
pub const ACCOUNT_PREFIX: &str = "mobile-pairing-";
/// The vault text's format.
const FORMAT: &str = "1";
/// A longer vault text is not ours (the longest real one is under 250 characters).
pub const MAXIMUM_TEXT: usize = 1024;

/// The three secrets of a [`PairingIdentity`].
#[derive(Clone, PartialEq, Eq)]
pub struct Secrets {
    pub pairing_key: String,
    pub host_token: String,
    pub secret_key: [u8; 32],
}

/// Secrets never reach a log.
impl fmt::Debug for Secrets {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Secrets")
    }
}

fn base64url(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b == b'-'
}

impl Secrets {
    pub fn of(identity: &PairingIdentity) -> Secrets {
        Secrets { pairing_key: identity.pairing_key.clone(), host_token: identity.host_token.clone(), secret_key: identity.secret_key }
    }

    /// `1.<pairing key>.<host token>.<secret key as base64url without padding>`: none of the parts holds a `.`.
    pub fn encode(&self) -> String {
        let secret = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(self.secret_key);
        format!("{FORMAT}.{}.{}.{secret}", self.pairing_key, self.host_token)
    }

    /// The vault text back, None unless every part has its shape: the pairing key 43-128 base64url characters, the host token 32-64
    /// hex digits (the Mac files' rule; Windows' stricter one is checked with the rest of its file), the secret key 32 bytes.
    pub fn decode(text: &str) -> Option<Secrets> {
        if text.len() > MAXIMUM_TEXT {
            return None;
        }
        let mut parts = text.split('.');
        let (format, pairing_key, host_token, secret) = (parts.next()?, parts.next()?, parts.next()?, parts.next()?);
        if format != FORMAT
            || parts.next().is_some()
            || !(43..=128).contains(&pairing_key.len())
            || !pairing_key.bytes().all(base64url)
            || !(32..=64).contains(&host_token.len())
            || !host_token.bytes().all(|b| b.is_ascii_hexdigit())
            || secret.len() != 43
        {
            return None;
        }
        let secret_key: [u8; 32] = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(secret).ok()?.try_into().ok()?;
        Some(Secrets { pairing_key: pairing_key.to_string(), host_token: host_token.to_string(), secret_key })
    }
}

/// Whether `value` is an account this module makes: [`ACCOUNT_PREFIX`] and 32 lower-case hex digits.
pub fn account(value: &str) -> bool {
    value
        .strip_prefix(ACCOUNT_PREFIX)
        .is_some_and(|hex| hex.len() == 32 && hex.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)))
}

/// A new account: 16 random bytes.
pub fn new_account() -> String {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).expect("the OS random source");
    let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    format!("{ACCOUNT_PREFIX}{hex}")
}

/// The placeholder a file holds where a secret stood.
pub fn placeholder(account: &str) -> String {
    format!("{PLACEHOLDER_PREFIX}{account}")
}

/// The account a placeholder names (surrounding white space ignored), None for any other text (a secret, damage).
pub fn placeholder_account(text: &str) -> Option<&str> {
    text.trim().strip_prefix(PLACEHOLDER_PREFIX).filter(|a| account(a))
}

/// A vault item ready for a write of the files: the one that already holds the secrets, or a new one (deleted again if the files
/// cannot be written).
#[derive(Debug)]
pub struct Prepared {
    pub account: String,
    fresh: bool,
}

/// A store's way to its vault: none (tests, a parity check: every secret stays in the files), or one, with the item the files name
/// now and its secrets, so a save that changes no secret neither reads nor writes the vault.
pub struct VaultSlot {
    vault: Option<Arc<dyn SecretVault>>,
    current: Mutex<Option<(String, Secrets)>>,
    off: AtomicBool,
}

impl fmt::Debug for VaultSlot {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("VaultSlot").field("vault", &self.vault.is_some()).field("off", &self.off.load(Ordering::Relaxed)).finish()
    }
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

impl VaultSlot {
    /// No vault: the secrets stay in the files.
    pub fn none() -> VaultSlot {
        VaultSlot { vault: None, current: Mutex::new(None), off: AtomicBool::new(false) }
    }

    pub fn new(vault: Arc<dyn SecretVault>) -> VaultSlot {
        VaultSlot { vault: Some(vault), current: Mutex::new(None), off: AtomicBool::new(false) }
    }

    /// Whether secrets found in the files should move to the vault now: there is one, and it has not refused in this run.
    pub fn active(&self) -> bool {
        self.vault.is_some() && !self.off.load(Ordering::SeqCst)
    }

    /// The secrets in the item a placeholder names. `Err` when there is no vault or the item is missing, damaged or cannot be read
    /// now (a locked or busy Keychain): the caller must not make a new identity over it.
    pub fn read(&self, account: &str) -> Result<Secrets> {
        let vault = self
            .vault
            .as_ref()
            .ok_or_else(|| PlatformError::Refused("the pairing secrets are in the vault, and this store has no vault".into()))?;
        let secrets = vault
            .load(account)
            .and_then(|text| Secrets::decode(&text))
            .ok_or_else(|| PlatformError::Refused("the pairing secrets could not be read from the vault".into()))?;
        *lock(&self.current) = Some((account.to_string(), secrets.clone()));
        Ok(secrets)
    }

    /// The item for a write of the files holding `secrets`: the current one when it holds them, else a new one written and read
    /// back. None when there is no vault or it refused (logged once; then not tried again in this run): the files keep the secrets.
    pub fn prepare(&self, secrets: &Secrets) -> Option<Prepared> {
        let vault = self.vault.as_ref()?;
        if let Some((account, held)) = lock(&self.current).as_ref()
            && held == secrets
        {
            return Some(Prepared { account: account.clone(), fresh: false });
        }
        if self.off.load(Ordering::SeqCst) {
            return None;
        }
        let account = new_account();
        let stored = vault.save(&account, &secrets.encode()).map_err(|error| error.to_string()).and_then(|()| {
            match vault.load(&account).and_then(|text| Secrets::decode(&text)) {
                Some(back) if back == *secrets => Ok(()),
                _ => Err("what it gave back is not what was saved".to_string()),
            }
        });
        match stored {
            Ok(()) => Some(Prepared { account, fresh: true }),
            Err(why) => {
                let _ = vault.delete(&account);
                if !self.off.swap(true, Ordering::SeqCst) {
                    // The vault's error names a status, never a secret or the account.
                    eprintln!("mighty: the pairing secrets stay in the pairing files this run: the vault refused them ({why})");
                }
                None
            }
        }
    }

    /// The files were written: naming `prepared`'s item, or (None) holding the secrets themselves. The item they named before is
    /// deleted when it is no longer theirs.
    pub fn committed(&self, prepared: Option<Prepared>, secrets: &Secrets) {
        let mut current = lock(&self.current);
        let old = current.take().map(|(account, _)| account);
        if let Some(prepared) = &prepared {
            *current = Some((prepared.account.clone(), secrets.clone()));
        }
        drop(current);
        if let (Some(vault), Some(old)) = (&self.vault, old)
            && prepared.as_ref().is_none_or(|p| p.account != old)
        {
            let _ = vault.delete(&old);
        }
    }

    /// The files could not be written: a new item made for them goes again; the files still name the item they named.
    pub fn abandoned(&self, prepared: Option<Prepared>) {
        if let (Some(vault), Some(prepared)) = (&self.vault, prepared)
            && prepared.fresh
        {
            let _ = vault.delete(&prepared.account);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake::MemoryVault;

    fn secrets() -> Secrets {
        Secrets {
            pairing_key: "AQgPFh0kKzI5QEdOVVxjanF4f4aNlJuiqbC3vsXM09o".into(),
            host_token: "0123456789abcdef".repeat(4),
            secret_key: [7; 32],
        }
    }

    #[test]
    fn the_vault_text_round_trips_and_refuses_what_is_not_its_shape() {
        let text = secrets().encode();
        assert_eq!(Secrets::decode(&text), Some(secrets()));
        assert!(!format!("{:?}", secrets()).contains("AQgP"), "secrets stay out of Debug");
        for bad in [
            String::new(),
            text.replacen("1.", "2.", 1),
            format!("{text}.x"),
            text.replacen("AQgPFh0k", "AQgP/h0k", 1),
            text.replacen(&"0123456789abcdef".repeat(4), "xyz", 1),
            text[..text.len() - 1].to_string(),
        ] {
            assert_eq!(Secrets::decode(&bad), None, "{bad}");
        }
        let account = new_account();
        assert!(self::account(&account) && placeholder_account(&format!(" {}\n", placeholder(&account))) == Some(account.as_str()));
        assert_eq!(placeholder_account("vault:mobile-pairing-short"), None);
        assert_eq!(placeholder_account("AQgPFh0kKzI5QEdOVVxjanF4f4aNlJuiqbC3vsXM09o"), None);
    }

    /// A change of secrets goes to a new item; the same secrets reuse the item; committing deletes the old item, abandoning deletes
    /// only a new one.
    #[test]
    fn items_are_new_per_change_and_the_old_one_goes_after_the_commit() {
        let memory = Arc::new(MemoryVault::default());
        let slot = VaultSlot::new(memory.clone());
        let first = slot.prepare(&secrets()).unwrap();
        let account = first.account.clone();
        slot.committed(Some(first), &secrets());
        let again = slot.prepare(&secrets()).unwrap();
        assert_eq!(again.account, account, "unchanged secrets reuse the item");
        slot.abandoned(Some(again));
        assert!(memory.load(&account).is_some(), "an item the files still name is not deleted");
        let rotated = Secrets { pairing_key: "B".repeat(43), ..secrets() };
        let next = slot.prepare(&rotated).unwrap();
        assert_ne!(next.account, account);
        let next_account = next.account.clone();
        slot.abandoned(Some(next));
        assert!(memory.load(&next_account).is_none() && memory.load(&account).is_some(), "a failed write drops only the new item");
        let next = slot.prepare(&rotated).unwrap();
        let next_account = next.account.clone();
        slot.committed(Some(next), &rotated);
        assert!(memory.load(&account).is_none() && memory.load(&next_account).is_some(), "the old item goes after the commit");
        assert_eq!(slot.read(&next_account).unwrap(), rotated);
        slot.committed(None, &rotated);
        assert!(memory.load(&next_account).is_none(), "secrets back in the files: the item goes");
        assert!(VaultSlot::none().prepare(&secrets()).is_none() && VaultSlot::none().read(&account).is_err());
    }
}
