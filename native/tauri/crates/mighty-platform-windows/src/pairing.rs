//! The phone pairing identity on Windows (C# `MobileIdentity`'s file half, R\MobileIdentity.cs:46-60, 155-179; plan §1.2, D14):
//! `<profile>\mobile-remote\mobile-identity.json`, read (at most 128 KiB, not a reparse point) and written atomically (`path.<guid>`,
//! CreateNew, flushed, moved over, the temporary file removed on failure) with mighty-platform's shared codec
//! (`pairing::identity_file`), in a folder that is not a reparse point and whose DACL is protected with one inherited FullControl
//! entry for the current user, who also owns it (`SecureDirectory`; dpapi.rs's folder ACL helper). Owner: W8-A.
//!
//! One difference from C#, on purpose: C#'s constructor secures (and creates) the folder before it reads; here only a save does, so a
//! read-only launch (verification mode) neither makes the folder nor touches its access list.
//!
//! A second one: given a vault (the app's: DPAPI, `dpapi.rs`), the pairing key, the host token and the relay secret key are kept
//! there and the file is version 2, naming the vault item instead (`identity_file::encode_vaulted`, `mighty_platform::pairing::
//! vaulted`); a version 1 file is read as before and its secrets moved out (they stay in it when DPAPI refuses). C# 0.4.x refuses a
//! version 2 file, so going back to it means pairing the phones again.

use std::fs;
use std::io::{Read, Write};
use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use mighty_platform::pairing::identity_file::{self, FILE, MAXIMUM_BYTES};
use mighty_platform::pairing::vaulted::{Secrets, VaultSlot};
use mighty_platform::{PairingIdentity, PairingStore, PlatformError, Result, SecretVault};

const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
/// `FILE_SHARE_NONE`: nobody else opens the temporary file while it is written.
const SHARE_NONE: u32 = 0;

/// `mobile-identity.json` in `folder` (the profile's `mobile-remote`).
#[derive(Debug)]
pub struct WindowsPairing {
    folder: PathBuf,
    /// Where the secrets go when there is a vault.
    vault: VaultSlot,
}

impl WindowsPairing {
    /// The secrets in the file (C#'s version 1).
    pub fn new(folder: PathBuf) -> WindowsPairing {
        WindowsPairing { folder, vault: VaultSlot::none() }
    }

    /// The secrets in `vault`, the file version 2; a version 1 file's secrets moved there when it is read.
    pub fn with_vault(folder: PathBuf, vault: Arc<dyn SecretVault>) -> WindowsPairing {
        WindowsPairing { folder, vault: VaultSlot::new(vault) }
    }

    fn file(&self) -> PathBuf {
        self.folder.join(FILE)
    }
}

fn linked(path: &Path) -> Result<bool> {
    let metadata = fs::symlink_metadata(path)?;
    Ok(metadata.file_type().is_symlink() || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0)
}

/// `SecureDirectory`: the folder made, refused when it is a link, and given this user alone (owner and one inherited FullControl
/// entry, inheritance from above cut).
fn secure_directory(folder: &Path) -> Result<()> {
    fs::create_dir_all(folder)?;
    if linked(folder)? {
        return Err(PlatformError::Refused("Refusing linked mobile identity directory.".into()));
    }
    let user = crate::dpapi::Sid::current_user()?;
    crate::dpapi::user_only(folder, &user)
}

impl PairingStore for WindowsPairing {
    fn load(&self) -> Result<Option<PairingIdentity>> {
        let file = self.file();
        if !self.folder.exists() || !file.exists() {
            return Ok(None);
        }
        if linked(&self.folder)? {
            return Err(PlatformError::Refused("Refusing linked mobile identity directory.".into()));
        }
        if fs::metadata(&file)?.len() > MAXIMUM_BYTES || linked(&file)? {
            return Err(PlatformError::Invalid("Invalid mobile identity file.".into()));
        }
        let mut bytes = Vec::new();
        fs::File::open(&file)?.take(MAXIMUM_BYTES + 1).read_to_end(&mut bytes)?;
        if bytes.len() as u64 > MAXIMUM_BYTES {
            return Err(PlatformError::Invalid("Invalid mobile identity file.".into()));
        }
        // `File.ReadAllText` drops a UTF-8 byte order mark.
        let text = std::str::from_utf8(bytes.strip_prefix(b"\xef\xbb\xbf").unwrap_or(&bytes))
            .map_err(|_| PlatformError::Invalid("Invalid mobile identity.".into()))?;
        // Version 2: the secrets from the vault, an item that cannot be read an error (never a new identity).
        let (identity, account) = identity_file::decode_stored(text, |account| self.vault.read(account))?;
        // Secrets found in the file move to the vault; when that fails they stay, and the identity is answered all the same.
        if account.is_none() && self.vault.active() {
            let _ = self.write_identity(&identity);
        }
        Ok(Some(identity))
    }

    fn save(&self, identity: &PairingIdentity) -> Result<()> {
        self.write_identity(identity)
    }
}

impl WindowsPairing {
    /// The file for `identity`: version 2 naming the vault item that holds its secrets when a vault is configured (failing closed when
    /// the vault refuses, and deleting a fresh item if writing the file fails), else version 1 with the secrets for unvaulted stores.
    fn write_identity(&self, identity: &PairingIdentity) -> Result<()> {
        let secrets = Secrets::of(identity);
        let prepared = self.vault.prepare(&secrets)?;
        let text = match &prepared {
            Some(prepared) => identity_file::encode_vaulted(identity, &prepared.account),
            None => identity_file::encode(identity),
        };
        match secure_directory(&self.folder).and(text).and_then(|text| self.write(&text)) {
            Ok(()) => {
                self.vault.committed(prepared, &secrets);
                Ok(())
            }
            Err(error) => {
                self.vault.abandoned(prepared);
                Err(error)
            }
        }
    }

    /// `Save`: beside the file as `mobile-identity.json.<guid>` (created new, nobody else sharing it), flushed to the disk, then moved
    /// over the file; the temporary file removed when anything failed.
    fn write(&self, text: &str) -> Result<()> {
        let temporary = self.folder.join(format!("{FILE}.{}", guid()));
        let written = (|| -> std::io::Result<()> {
            let mut file = fs::OpenOptions::new().write(true).create_new(true).share_mode(SHARE_NONE).open(&temporary)?;
            file.write_all(text.as_bytes())?;
            file.sync_all()?;
            drop(file);
            fs::rename(&temporary, self.file())
        })();
        if written.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        Ok(written?)
    }
}

/// `Guid.NewGuid().ToString("N")`: 32 random lower-case hex digits.
fn guid() -> String {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).expect("the OS random source");
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use mighty_platform::PairedDevice;

    fn temporary(name: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        std::env::temp_dir().join(format!("mighty-pairing-{name}-{}-{nanos}", std::process::id()))
    }

    fn identity() -> PairingIdentity {
        PairingIdentity {
            host_id: "4e0c0a3f-8a59-4a0f-9a57-0a3c1b2d3e4f".into(),
            host_token: "0123456789abcdef".repeat(4),
            pairing_key: "AQgPFh0kKzI5QEdOVVxjanF4f4aNlJuiqbC3vsXM09o".into(),
            secret_key: [9; 32],
            devices: vec![PairedDevice {
                id: "legacy".into(),
                name: "Legacy app".into(),
                token_hash: None,
                first_seen: "2026-10-23T01:02:03.0000000+00:00".into(),
                last_seen: "2026-10-23T01:02:03.0000000+00:00".into(),
                push: None,
            }],
        }
    }

    /// Nothing is read or made before the first save; the save writes C#'s bytes atomically into a folder only this user may open,
    /// leaving no temporary file; the file reads back.
    #[test]
    fn the_identity_round_trips_through_an_owner_only_folder() {
        let root = temporary("roundtrip");
        let folder = root.join(identity_file::FOLDER);
        let store = WindowsPairing::new(folder.clone());
        assert!(store.load().unwrap().is_none() && !folder.exists(), "a load makes nothing");
        store.save(&identity()).unwrap();
        assert_eq!(store.load().unwrap(), Some(identity()));
        assert_eq!(fs::read_to_string(folder.join(FILE)).unwrap(), identity_file::encode(&identity()).unwrap());
        let names: Vec<String> = fs::read_dir(&folder).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
        assert_eq!(names, vec![FILE.to_string()], "no temporary file is left");
        let user = crate::dpapi::Sid::current_user().unwrap();
        assert!(crate::dpapi::owned_by(&folder, &user).unwrap(), "the folder is this user's");
        let mut changed = identity();
        changed.pairing_key = "B".repeat(43);
        store.save(&changed).unwrap();
        assert_eq!(store.load().unwrap().unwrap().pairing_key, "B".repeat(43), "a second save replaces the file");
        let _ = fs::remove_dir_all(&root);
    }

    /// A refusing vault (DPAPI saying no).
    struct RefusingVault;

    impl SecretVault for RefusingVault {
        fn load(&self, _id: &str) -> Option<String> {
            None
        }
        fn save(&self, _id: &str, _secret: &str) -> Result<()> {
            Err(PlatformError::Refused("DPAPI: refused".into()))
        }
        fn delete(&self, _id: &str) -> Result<()> {
            Ok(())
        }
    }

    /// A version 1 file read with a vault: its secrets move there and the file becomes version 2 without them; it reads back at the
    /// next launch; a rotation goes to a new item; without the item the load is an error and the file stays.
    #[test]
    fn a_version_1_file_moves_its_secrets_to_the_vault() {
        let root = temporary("vault");
        let folder = root.join(identity_file::FOLDER);
        WindowsPairing::new(folder.clone()).save(&identity()).unwrap();
        let vault = Arc::new(mighty_platform::fake::MemoryVault::default());
        let store = WindowsPairing::with_vault(folder.clone(), vault.clone());
        assert_eq!(store.load().unwrap(), Some(identity()));
        let text = fs::read_to_string(folder.join(FILE)).unwrap();
        assert!(text.starts_with("{\"version\":2,") && !text.contains(&identity().pairing_key) && !text.contains(&identity().host_token));
        assert_eq!(WindowsPairing::with_vault(folder.clone(), vault.clone()).load().unwrap(), Some(identity()));
        let rotated = PairingIdentity { pairing_key: "B".repeat(43), ..identity() };
        store.save(&rotated).unwrap();
        assert_eq!(WindowsPairing::with_vault(folder.clone(), vault.clone()).load().unwrap(), Some(rotated));
        let before = fs::read(folder.join(FILE)).unwrap();
        assert!(WindowsPairing::with_vault(folder.clone(), Arc::new(mighty_platform::fake::MemoryVault::default())).load().is_err());
        assert!(WindowsPairing::new(folder.clone()).load().is_err());
        assert_eq!(fs::read(folder.join(FILE)).unwrap(), before, "never a new identity over it");
        let _ = fs::remove_dir_all(&root);
    }

    /// P0-SEC-3: a vault that refuses fails closed on save instead of writing plaintext secrets to disk.
    #[test]
    fn a_refusing_vault_refuses_to_write_plaintext_secrets() {
        let fresh = temporary("vault-refuses-fresh");
        let fresh_folder = fresh.join(identity_file::FOLDER);
        let err = WindowsPairing::with_vault(fresh_folder.clone(), Arc::new(RefusingVault)).save(&identity()).unwrap_err();
        assert!(matches!(err, PlatformError::Refused(_)), "expected Refused, got {err:?}");
        assert!(!fresh_folder.exists(), "a refused save must not write plaintext files");

        let root = temporary("vault-refuses");
        let folder = root.join(identity_file::FOLDER);
        WindowsPairing::new(folder.clone()).save(&identity()).unwrap();
        let before = fs::read_to_string(folder.join(FILE)).unwrap();
        let store = WindowsPairing::with_vault(folder.clone(), Arc::new(RefusingVault));
        assert_eq!(store.load().unwrap(), Some(identity()));
        assert_eq!(fs::read_to_string(folder.join(FILE)).unwrap(), before);
        let rotated = PairingIdentity { pairing_key: "B".repeat(43), ..identity() };
        assert!(matches!(store.save(&rotated), Err(PlatformError::Refused(_))));
        assert_eq!(fs::read_to_string(folder.join(FILE)).unwrap(), before, "refused save must not overwrite file with plaintext");
        let _ = fs::remove_dir_all(&root);
    }

    /// A file over 128 KiB or one that does not decode is an error (the caller decides), never a silent start over.
    #[test]
    fn a_damaged_or_oversized_file_is_refused() {
        let root = temporary("damaged");
        let folder = root.join(identity_file::FOLDER);
        fs::create_dir_all(&folder).unwrap();
        let store = WindowsPairing::new(folder.clone());
        fs::write(folder.join(FILE), "not json").unwrap();
        assert!(store.load().is_err());
        fs::write(folder.join(FILE), vec![b' '; MAXIMUM_BYTES as usize + 1]).unwrap();
        assert!(store.load().is_err());
        let mut text = vec![0xef, 0xbb, 0xbf];
        text.extend_from_slice(identity_file::encode(&identity()).unwrap().as_bytes());
        fs::write(folder.join(FILE), text).unwrap();
        assert_eq!(store.load().unwrap(), Some(identity()), "a byte order mark is dropped as File.ReadAllText drops it");
        let _ = fs::remove_dir_all(&root);
    }
}
