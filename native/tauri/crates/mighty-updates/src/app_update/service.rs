//! Check, download and the kept update (C# `AppUpdateService.cs`; plan §1.3). Owner: W9-A.
//!
//! Over mighty-platform's `UpdateTransport` (no proxy; redirects come back to this code): the check follows 301/302/303/307/308 by hand
//! while each next hop is https (`redirectNotHTTPS`), at most [`MAX_REDIRECTS`] (`tooManyRedirects`), a non-2xx is `manifestHTTP`, the
//! body is capped at 256 KiB; no key is `NoPublicKeyNotice`. The download refuses an asset over [`MAX_PACKAGE_BYTES`], deletes every other
//! version folder under `<data>\updates`, reuses `updates\<v>\MightyCode-windows.zip` when its size and SHA-256 match, else writes it
//! counting (over the asset's size or the cap: `packageTooLarge`), reports progress once per whole percent (at most 0.99 until done),
//! checks the size (`packageSize`) and the SHA-256 (`packageDigest`), and deletes the version folder on any failure or cancel (also when
//! the download's future is dropped half way); 30 minutes at most. Tests use `mighty_platform::fake::FakeUpdateTransport`, never the
//! real update address.
//!
//! A transport that runs out of time is a cancel, as C#'s `HttpClient` timeout is (`TaskCanceledException`, an
//! `OperationCanceledException`): the coordinator goes back without an error.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use mighty_platform::{PlatformError, UpdateTransport};
use sha2::{Digest, Sha256};
use tokio_util::sync::CancellationToken;

use super::UpdateError;
use super::manifest::{Asset, Availability, MAX_MANIFEST_BYTES};

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

/// A download's whole time (C#: 30 minutes, `HttpClient.Timeout`, which also bounds the check).
pub const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(30 * 60);

/// Why a check or a download did not end with an answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServiceError {
    /// Cancelled (C#'s `OperationCanceledException`): no error is shown.
    Cancelled,
    /// Refused, with C#'s sentence.
    Refused(UpdateError),
}

impl From<UpdateError> for ServiceError {
    fn from(error: UpdateError) -> ServiceError {
        ServiceError::Refused(error)
    }
}

fn plain(error: impl std::fmt::Display) -> ServiceError {
    ServiceError::Refused(UpdateError::Plain(error.to_string()))
}

/// A transport failure: out of time or cancelled is a cancel; anything else its own words (the port never puts an address in them).
fn transport_error(error: PlatformError, too_large: &'static str) -> ServiceError {
    match error {
        PlatformError::TimedOut(_) => ServiceError::Cancelled,
        PlatformError::Refused(reason) if reason == "cancelled" => ServiceError::Cancelled,
        PlatformError::TooLarge(_) => ServiceError::Refused(UpdateError::key(too_large)),
        other => plain(other),
    }
}

/// C# `RequireHttps(value, message)`.
fn require_https(value: Option<&str>, refusal: &'static str) -> Result<String, ServiceError> {
    value.filter(|value| super::address::allowed(value)).map(String::from).ok_or_else(|| UpdateError::key(refusal).into())
}

fn is_redirect(status: u16) -> bool {
    matches!(status, 301 | 302 | 303 | 307 | 308)
}

/// SHA-256 of the bytes on disk as lower-case hex, read in 1 MiB pieces (C# `AppUpdateService.Sha256Async`).
pub fn sha256(path: &Path) -> std::io::Result<String> {
    let mut file = std::fs::File::open(path)?;
    let mut digest = Sha256::new();
    let mut buffer = vec![0u8; 1 << 20];
    loop {
        match file.read(&mut buffer)? {
            0 => break,
            read => digest.update(&buffer[..read]),
        }
    }
    Ok(digest.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

/// [`sha256`] off the async thread (C# hashes on the thread pool).
async fn sha256_off_thread(path: PathBuf) -> std::io::Result<String> {
    tokio::task::spawn_blocking(move || sha256(&path)).await.unwrap_or_else(|join| Err(std::io::Error::other(join.to_string())))
}

/// C#'s `MatchesAsync`: the file holds exactly the asset's bytes (size, then SHA-256); an unreadable one does not.
async fn matches(path: &Path, asset: &Asset) -> bool {
    if !path.is_file() || std::fs::metadata(path).map(|m| m.len()).ok() != Some(asset.size) {
        return false;
    }
    sha256_off_thread(path.to_path_buf()).await.is_ok_and(|digest| digest.eq_ignore_ascii_case(&asset.sha256))
}

/// Deletes the version folder when a download ends without its package verified, also when its future is dropped half way.
struct VersionFolderGuard {
    folder: PathBuf,
    armed: bool,
}

impl Drop for VersionFolderGuard {
    fn drop(&mut self) {
        if self.armed {
            super::housekeeping::remove_folder(&self.folder);
        }
    }
}

/// The check, the download and the staging of one profile's updates (C# `AppUpdateService`).
pub struct Service {
    transport: Arc<dyn UpdateTransport>,
    updates_directory: PathBuf,
    public_key: Option<[u8; 32]>,
    downloading: Mutex<Option<CancellationToken>>,
}

impl std::fmt::Debug for Service {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Service")
            .field("updates_directory", &self.updates_directory)
            .field("verifies_signatures", &self.verifies_signatures())
            .finish_non_exhaustive()
    }
}

impl Service {
    /// The service for the profile whose data folder is `data_directory`, with the build's key (Windows rule 1: without one it never
    /// checks).
    pub fn new(transport: Arc<dyn UpdateTransport>, data_directory: &Path, public_key: Option<[u8; 32]>) -> Service {
        Service { transport, updates_directory: data_directory.join(UPDATES_FOLDER), public_key, downloading: Mutex::new(None) }
    }

    /// `<data>\updates` (C# `UpdatesDirectory`).
    pub fn updates_directory(&self) -> &Path {
        &self.updates_directory
    }

    /// The transport the check and the download go through.
    pub fn transport(&self) -> &Arc<dyn UpdateTransport> {
        &self.transport
    }

    /// C# `VerifiesSignatures`.
    pub fn verifies_signatures(&self) -> bool {
        self.public_key.is_some()
    }

    /// C# `CheckAsync(manifestUrl, currentVersion)`: the signed manifest beside this build's version.
    pub async fn check(&self, manifest_url: Option<&str>, current_version: &str) -> Result<Availability, ServiceError> {
        let Some(public_key) = self.public_key else {
            return Err(UpdateError::key("settings.appUpdate.noPublicKeyNoticeWindows").into());
        };
        let mut url = require_https(manifest_url, "appUpdate.error.manifestNotHTTPS")?;
        let mut hop = 0;
        let response = loop {
            let response = self
                .transport
                .get_capped(&url, MAX_MANIFEST_BYTES, DOWNLOAD_TIMEOUT)
                .await
                .map_err(|error| transport_error(error, "appUpdate.error.manifestFileTooLarge"))?;
            match response.location.as_deref().filter(|_| is_redirect(response.status)) {
                Some(location) if hop < MAX_REDIRECTS => {
                    url = require_https(Some(&super::address::resolve(&url, location)), "appUpdate.error.redirectNotHTTPS")?;
                    hop += 1;
                }
                Some(location) => {
                    // The sixth redirect: C# checks its address before it gives up on the count.
                    require_https(Some(&super::address::resolve(&url, location)), "appUpdate.error.redirectNotHTTPS")?;
                    return Err(UpdateError::key("appUpdate.error.tooManyRedirects").into());
                }
                None => break response,
            }
        };
        if !(200..300).contains(&response.status) {
            return Err(
                UpdateError::Key { key: "appUpdate.error.manifestHTTP", args: vec![("status", response.status.to_string())] }.into()
            );
        }
        if response.body.len() > MAX_MANIFEST_BYTES {
            return Err(UpdateError::key("appUpdate.error.manifestFileTooLarge").into());
        }
        let manifest = super::manifest::parse(&response.body, &public_key)?;
        Ok(Availability { current: current_version.to_string(), manifest })
    }

    /// C# `DownloadAsync(asset, version, progress, cancellation)`: the verified package's path. `progress` gets fractions in [0, 1]
    /// (once per whole percent, at most 0.99 while the bytes arrive, then 1). Cancelled through `cancel` or [`Service::cancel_download`].
    pub async fn download(
        &self,
        asset: &Asset,
        version: &str,
        progress: Arc<dyn Fn(f64) + Send + Sync>,
        cancel: &CancellationToken,
    ) -> Result<PathBuf, ServiceError> {
        let url = require_https(Some(&asset.url), "appUpdate.error.packageNotHTTPS")?;
        if asset.size > MAX_PACKAGE_BYTES {
            return Err(UpdateError::key("appUpdate.error.packageTooLarge").into());
        }
        let name = super::version::normalized(version).ok_or_else(|| ServiceError::from(UpdateError::key("appUpdate.error.noVersion")))?;
        std::fs::create_dir_all(&self.updates_directory).map_err(plain)?;
        for entry in std::fs::read_dir(&self.updates_directory).map_err(plain)?.flatten() {
            if entry.path().is_dir() && entry.file_name().to_string_lossy() != name {
                let _ = std::fs::remove_dir_all(entry.path());
            }
        }

        let version_directory = self.updates_directory.join(&name);
        let destination = version_directory.join(PACKAGE_NAME);
        if matches(&destination, asset).await {
            progress(1.0);
            return Ok(destination);
        }
        if version_directory.is_dir() {
            std::fs::remove_dir_all(&version_directory).map_err(plain)?;
        }
        std::fs::create_dir_all(&version_directory).map_err(plain)?;

        let mut guard = VersionFolderGuard { folder: version_directory, armed: true };
        let owned = cancel.child_token();
        *self.downloading.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(owned.clone());
        let written = self.write_to_disk(url, &destination, asset.size, progress.clone(), &owned).await;
        *self.downloading.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
        written?;

        let received = std::fs::metadata(&destination).map_err(plain)?.len();
        if received != asset.size {
            return Err(UpdateError::Key {
                key: "appUpdate.error.packageSize",
                args: vec![("received", received.to_string()), ("expected", asset.size.to_string())],
            }
            .into());
        }
        let digest = sha256_off_thread(destination.clone()).await.map_err(plain)?;
        if !digest.eq_ignore_ascii_case(&asset.sha256) {
            return Err(UpdateError::key("appUpdate.error.packageDigest").into());
        }
        guard.armed = false;
        progress(1.0);
        Ok(destination)
    }

    /// C# `CancelDownload`: the download under way stops (its folder is deleted).
    pub fn cancel_download(&self) {
        if let Some(token) = self.downloading.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).as_ref() {
            token.cancel();
        }
    }

    /// C#'s `WriteToDiskAsync`: the package's bytes to `destination`, following https redirects, refusing past the asset's size or
    /// the cap, with C#'s progress steps.
    async fn write_to_disk(
        &self,
        mut url: String,
        destination: &Path,
        expected: u64,
        progress: Arc<dyn Fn(f64) + Send + Sync>,
        cancel: &CancellationToken,
    ) -> Result<(), ServiceError> {
        let reported = Arc::new(Mutex::new(-1i64));
        let on_bytes: mighty_platform::DownloadProgress = {
            let reported = reported.clone();
            Arc::new(move |written: u64| {
                if expected == 0 {
                    return;
                }
                let fraction = (written as f64 / expected as f64).min(0.99);
                let percent = (fraction * 100.0) as i64;
                let mut last = reported.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
                if *last != percent {
                    *last = percent;
                    drop(last);
                    progress(fraction);
                }
            })
        };
        let mut hop = 0;
        loop {
            if cancel.is_cancelled() {
                return Err(ServiceError::Cancelled);
            }
            let flag = Arc::new(AtomicBool::new(false));
            let request = mighty_platform::DownloadRequest {
                url: url.clone(),
                destination: destination.to_path_buf(),
                limit: expected.min(MAX_PACKAGE_BYTES),
                timeout: DOWNLOAD_TIMEOUT,
                progress: on_bytes.clone(),
                cancel: flag.clone(),
            };
            let mut transfer = self.transport.download(request);
            // A cancel sets the transport's own flag and lets it end on its own, so its file is closed before the folder goes.
            let answer = tokio::select! {
                answer = &mut transfer => answer,
                () = cancel.cancelled() => {
                    flag.store(true, Ordering::SeqCst);
                    let _ = transfer.await;
                    return Err(ServiceError::Cancelled);
                }
            };
            let answer = answer.map_err(|error| transport_error(error, "appUpdate.error.packageTooLarge"))?;
            match answer.location.as_deref().filter(|_| is_redirect(answer.status)) {
                Some(location) if hop < MAX_REDIRECTS => {
                    url = require_https(Some(&super::address::resolve(&url, location)), "appUpdate.error.redirectNotHTTPS")?;
                    hop += 1;
                }
                Some(location) => {
                    require_https(Some(&super::address::resolve(&url, location)), "appUpdate.error.redirectNotHTTPS")?;
                    return Err(UpdateError::key("appUpdate.error.tooManyRedirects").into());
                }
                None if (200..300).contains(&answer.status) => return Ok(()),
                None => {
                    return Err(
                        UpdateError::Key { key: "appUpdate.error.packageHTTP", args: vec![("status", answer.status.to_string())] }.into()
                    );
                }
            }
        }
    }

    /// C# `KeptVersion(currentVersion)`: the newest folder version above `current_version` whose package and staged tree (exactly one
    /// `MightyCode.exe`) an earlier run left under `updates`; None when there is none or the folder cannot be read.
    pub fn kept_version(&self, current_version: &str) -> Option<String> {
        let entries = std::fs::read_dir(&self.updates_directory).ok()?;
        let mut kept: Vec<String> = Vec::new();
        for entry in entries {
            let entry = entry.ok()?;
            let folder = entry.path();
            if !folder.is_dir() {
                continue;
            }
            let Some(version) = super::version::normalized(&entry.file_name().to_string_lossy()) else { continue };
            if super::version::compare(&version, current_version) != std::cmp::Ordering::Greater
                || !folder.join(PACKAGE_NAME).is_file()
                || !folder.join(STAGED_FOLDER).is_dir()
            {
                continue;
            }
            let executables = count_named(&folder.join(STAGED_FOLDER), super::EXECUTABLE_NAME).ok()?;
            if executables == 1 {
                kept.push(version);
            }
        }
        kept.into_iter().max_by(|a, b| super::version::compare(a, b))
    }
}

/// How many files named `name` (without case) lie under `root`.
fn count_named(root: &Path, name: &str) -> std::io::Result<usize> {
    let mut count = 0;
    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        for entry in std::fs::read_dir(&directory)? {
            let path = entry?.path();
            if path.is_dir() {
                pending.push(path);
            } else if super::dotnet::eq_ignore_case(&super::dotnet::file_name(&path), name) {
                count += 1;
            }
        }
    }
    Ok(count)
}
