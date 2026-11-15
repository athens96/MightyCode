//! Check, download and the kept update (C# `AppUpdateService.cs`; plan §1.3). Owner: W9-A.
//!
//! Over mighty-platform's `UpdateTransport` (no proxy; redirects come back to this code): the check follows 301/302/303/307/308 by hand
//! while each next hop is https (`redirectNotHTTPS`), at most [`MAX_REDIRECTS`] (`tooManyRedirects`), a non-2xx is `manifestHTTP`, the
//! body is capped at 256 KiB; no key is `NoPublicKeyNotice`. The download refuses an asset over [`MAX_PACKAGE_BYTES`], deletes every other
//! version folder under `<data>\updates`, reuses `updates\<v>\MightyCode-windows.zip` when its size and SHA-256 match, else writes it
//! counting (over the asset's size or the cap: `packageTooLarge`), reports progress once per whole percent (at most 0.99 until done),
//! checks the size (`packageSize`) and the SHA-256 (`packageDigest`), and deletes the version folder on any failure or cancel; 30
//! minutes at most. Tests use `mighty_platform::fake::FakeUpdateTransport`, never the real update address.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use mighty_platform::UpdateTransport;

/// The largest package (C# `AppUpdateService.MaxPackageBytes`: 512 MiB).
pub const MAX_PACKAGE_BYTES: u64 = 512 * 1024 * 1024;

/// The downloaded package's name in its version folder (C# `PackageName`).
pub const PACKAGE_NAME: &str = "MightyCode-windows.zip";

/// The folder under the data folder that holds the version folders (C# `UpdatesDirectory`).
pub const UPDATES_FOLDER: &str = "updates";

/// The unpacked package beside it (`updates\<version>\staged`).
pub const STAGED_FOLDER: &str = "staged";

/// Redirects followed at most (C# `MaxRedirects`).
pub const MAX_REDIRECTS: usize = 5;

/// A download's whole time (C#: 30 minutes).
pub const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(30 * 60);

/// The check, the download and the staging of one profile's updates (C# `AppUpdateService`). Step 0: the type and its folders; the
/// operations are W9-A's.
pub struct Service {
    transport: Arc<dyn UpdateTransport>,
    updates_directory: PathBuf,
}

impl Service {
    /// The service for the profile whose data folder is `data_directory`.
    pub fn new(transport: Arc<dyn UpdateTransport>, data_directory: &Path) -> Service {
        Service { transport, updates_directory: data_directory.join(UPDATES_FOLDER) }
    }

    /// `<data>\updates` (C# `UpdatesDirectory`).
    pub fn updates_directory(&self) -> &Path {
        &self.updates_directory
    }

    /// The transport the check and the download go through.
    pub fn transport(&self) -> &Arc<dyn UpdateTransport> {
        &self.transport
    }
}
