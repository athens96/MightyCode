//! M-B: the phone's pairing identity in the Swift app's files under `<data>/mobile-remote/` (`mighty_platform::pairing::four_files`,
//! shared code chosen by `PlatformProfile::pairing_layout`), with Swift's modes: the folder 0700, every file 0600 from its creation
//! (`createFile(atPath:contents:attributes: [.posixPermissions: 0o600])`). Swift `MobileRemoteService.swift:349-442`,
//! `MobileDeviceRegistry.swift:428-486`, `RelayChannel.swift:86-104`; contract `mighty_platform::pairing` (`PairingStore`).
//! Swift-written files read here, and what this writes reads back in Swift (`tools/parity-mac pairing`).

use std::fs;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::Path;
use std::sync::Arc;

use mighty_platform::pairing::four_files::{FOLDER, FourFiles, PrivateFiles};
use mighty_platform::{PairingStore, Paths};

/// The folder 0700 and each file 0600.
pub struct OwnerOnly;

impl PrivateFiles for OwnerOnly {
    /// Made 0700 from the start (`DirBuilder` with the mode), and an existing one set to 0700 only after it is known not to be a
    /// link (`chmod` follows one).
    fn private_folder(&self, folder: &Path) -> std::io::Result<()> {
        fs::DirBuilder::new().recursive(true).mode(0o700).create(folder)?;
        self.tighten(folder)
    }
    fn private_file(&self, path: &Path) -> std::io::Result<fs::File> {
        fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(path)
    }
    /// 0700 for a folder, 0600 for a file; a link is refused, never followed.
    fn tighten(&self, path: &Path) -> std::io::Result<()> {
        let meta = fs::symlink_metadata(path)?;
        if meta.file_type().is_symlink() {
            return Err(std::io::Error::other("a link where a private file or folder should be"));
        }
        fs::set_permissions(path, fs::Permissions::from_mode(if meta.is_dir() { 0o700 } else { 0o600 }))
    }
}

/// The Mac's `PairingStore` over the profile's data folder.
pub fn pairing_store(paths: &dyn Paths) -> Arc<dyn PairingStore> {
    Arc::new(FourFiles::new(paths.data_dir().join(FOLDER), Box::new(OwnerOnly)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use mighty_platform::pairing::four_files::{DEVICES, PAIRING_KEY};
    use mighty_platform::{PairedDevice, PairingIdentity};

    fn mode(path: &Path) -> u32 {
        fs::metadata(path).unwrap().permissions().mode() & 0o777
    }

    /// `MobileRemoteExtensionTests.swift:1148 "aDeviceTokenIsIssuedOnceAndOnlyItsHashIsKept"` (its file half; the token logic is
    /// mighty-remote's `Identity`, tests/four_files.rs): the list is 0600 in a 0700 folder and holds the hash, never a token.
    #[test]
    fn the_files_are_owner_only_in_an_owner_only_folder() {
        let root = std::env::temp_dir().join(format!("mighty-mac-pairing-{}-{}", std::process::id(), line!()));
        let folder = root.join(FOLDER);
        let store = FourFiles::new(folder.clone(), Box::new(OwnerOnly));
        store
            .save(&PairingIdentity {
                host_id: "4e0c0a3f-8a59-4a0f-9a57-0a3c1b2d3e4f".into(),
                host_token: "ab".repeat(32),
                pairing_key: "A".repeat(43),
                secret_key: [3; 32],
                devices: vec![PairedDevice {
                    id: "Zm9vYmFyYmF6cXV4MDA".into(),
                    name: "iPhone".into(),
                    token_hash: Some("5723c1b7256eba050781d42f7d4fd96201c4b75d7673b1a82ffbcba6dc2e1c23".into()),
                    first_seen: "2026-10-23T01:02:03.000Z".into(),
                    last_seen: "2026-10-23T01:02:03.000Z".into(),
                    push: None,
                }],
            })
            .unwrap();
        assert_eq!(mode(&folder), 0o700);
        for entry in fs::read_dir(&folder).unwrap() {
            assert_eq!(mode(&entry.unwrap().path()), 0o600);
        }
        assert!(folder.join(PAIRING_KEY).exists() && folder.join(DEVICES).exists());
        // A load makes what it reads owner-only again, as Swift does.
        fs::set_permissions(folder.join(PAIRING_KEY), fs::Permissions::from_mode(0o644)).unwrap();
        fs::set_permissions(&folder, fs::Permissions::from_mode(0o755)).unwrap();
        store.load().unwrap().unwrap();
        assert_eq!((mode(&folder.join(PAIRING_KEY)), mode(&folder)), (0o600, 0o700));
        // A corrupt list's bytes are kept aside owner-only too.
        fs::write(folder.join(DEVICES), "{ not json").unwrap();
        let read = store.load().unwrap().unwrap();
        store.save(&read).unwrap();
        let aside = fs::read_dir(&folder).unwrap().map(|e| e.unwrap().path()).find(|p| p.to_string_lossy().contains(".corrupt-")).unwrap();
        assert_eq!(mode(&aside), 0o600);
        let _ = fs::remove_dir_all(&root);
    }
}
