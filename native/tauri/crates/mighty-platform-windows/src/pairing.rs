//! The phone pairing identity on Windows (C# `MobileIdentity`'s file half, R\MobileIdentity.cs:46-60, 155-179; plan §1.2, D14):
//! `<profile>\mobile-remote\mobile-identity.json`, read (at most 128 KiB, not a reparse point) and written atomically (`path.<guid>`,
//! CreateNew, flushed, moved over, the temporary file removed on failure) with mighty-platform's shared codec
//! (`pairing::identity_file`), in a folder that is not a reparse point and whose DACL is protected with one inherited FullControl
//! entry for the current user, who also owns it (`SecureDirectory`; dpapi.rs's folder ACL helper). Owner: W8-A.
//!
//! At rest (W8 Step 0): nothing is read or written; both calls answer `Unsupported`, as the port did before W8.

use std::path::PathBuf;

use mighty_platform::{PairingIdentity, PairingStore, PlatformError, Result};

/// `mobile-identity.json` in `folder` (the profile's `mobile-remote`).
#[derive(Debug, Clone)]
pub struct WindowsPairing {
    folder: PathBuf,
}

impl WindowsPairing {
    pub fn new(folder: PathBuf) -> WindowsPairing {
        WindowsPairing { folder }
    }
}

impl PairingStore for WindowsPairing {
    fn load(&self) -> Result<Option<PairingIdentity>> {
        let _ = &self.folder;
        Err(PlatformError::unsupported("PairingStore", "load"))
    }
    fn save(&self, _identity: &PairingIdentity) -> Result<()> {
        Err(PlatformError::unsupported("PairingStore", "save"))
    }
}
