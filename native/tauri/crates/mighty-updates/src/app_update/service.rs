//! Check, download and the kept update (C# `AppUpdateService.cs`; plan §1.3). Owner: W9-A.
//!
//! Over mighty-platform's `UpdateTransport` (no proxy; redirects come back to this code): the check follows 301/302/303/307/308 by hand
//! while each next hop is https (`redirectNotHTTPS`), at most [`MAX_REDIRECTS`] (`tooManyRedirects`), a non-2xx is `manifestHTTP`, the
//! body is capped at 256 KiB; no key is `NoPublicKeyNotice`. The download refuses an asset over [`MAX_PACKAGE_BYTES`], deletes every other
//! version folder under `<data>\updates`, reuses `updates\<v>\MightyCode-windows.zip` when its size and SHA-256 match, else writes it
//! counting (over the asset's size or the cap: `packageTooLarge`), reports progress once per whole percent (at most 0.99 until done),
//! checks the size (`packageSize`) and the SHA-256 (`packageDigest`), and deletes the version folder on any failure or cancel (also when
//! the download's future is dropped half way, which also stops the transfer). The manifest request has C#'s 30 minutes; a package
//! download is stopped only when no byte has come for [`DOWNLOAD_STALL`] (C# does not bound the body at all). Tests use
//! `mighty_platform::fake::FakeUpdateTransport`, never the real update address.
//!
//! A manifest request that runs out of time is a cancel, as C#'s `HttpClient` timeout is (`TaskCanceledException`, an
//! `OperationCanceledException`): the coordinator goes back without an error.
//!
//! M-H: [`Service::with_profile`] gives the Mac's rules (Swift `AppUpdateService`): the package is `updates/<v>/MightyCode-macos.zip`, a
//! kept update's staged folder holds exactly one `.app`, and a build without a key refuses with `appUpdate.error.noPublicKey`.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use mighty_platform::profile::UpdatePackage;
use mighty_platform::{PlatformError, PlatformProfile, UpdateTransport};
use sha2::{Digest, Sha256};
use tokio_util::sync::CancellationToken;

use super::UpdateError;
use super::manifest::{Asset, Availability, MAX_MANIFEST_BYTES};

/// The largest package (C# `AppUpdateService.MaxPackageBytes`: 512 MiB).
pub const MAX_PACKAGE_BYTES: u64 = 512 * 1024 * 1024;

/// The downloaded package's name in its version folder (C# `PackageName`).
pub const PACKAGE_NAME: &str = "MightyCode-windows.zip";

/// The Mac's (Swift `AppUpdateService.packageName`).
pub const MAC_PACKAGE_NAME: &str = "MightyCode-macos.zip";

/// The package's name for `package`.
pub fn package_name(package: UpdatePackage) -> &'static str {
    match package {
        UpdatePackage::WindowsZip => PACKAGE_NAME,
        UpdatePackage::MacBundle => MAC_PACKAGE_NAME,
    }
}

/// What a build without a key says (the profile's `update_needs_public_key`): Windows' notice that the check is off, or the Mac's
/// refusal.
pub fn no_public_key(needs_public_key: bool) -> UpdateError {
    UpdateError::key(if needs_public_key { "settings.appUpdate.noPublicKeyNoticeWindows" } else { "appUpdate.error.noPublicKey" })
}

/// The folder under the data folder that holds the version folders (C# `UpdatesDirectory`).
pub const UPDATES_FOLDER: &str = "updates";

/// The unpacked package beside it (`updates\<version>\staged`).
pub const STAGED_FOLDER: &str = "staged";

/// Redirects followed at most (C# `MaxRedirects`).
pub const MAX_REDIRECTS: usize = 5;

/// The manifest request's time (C#: 30 minutes, `HttpClient.Timeout`). Running out of it is a cancel, as C#'s timeout is.
pub const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(30 * 60);

/// A package download stops when no byte has arrived for this long (`packageSize`, the bytes received so far). C# reads the body
/// with `ResponseHeadersRead`, which `HttpClient.Timeout` does not bound, so a stalled body would wait for ever there; a slow but
/// moving download is never cut off.
pub const DOWNLOAD_STALL: Duration = Duration::from_secs(60);

/// The bound the transport is given for a whole package download: long enough for any connection that keeps moving (the stall above
/// is what stops a dead one).
pub const PACKAGE_TIMEOUT: Duration = Duration::from_secs(24 * 60 * 60);

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

/// Sets the transport's cancel flag when the download's future is dropped before the transfer answered, so the transfer stops too.
struct CancelOnDrop {
    flag: Arc<AtomicBool>,
    armed: bool,
}

impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        if self.armed {
            self.flag.store(true, Ordering::SeqCst);
        }
    }
}

/// Empties the service's slot for the download under way however the download ends (a dropped future included).
struct ClearSlot<'a>(&'a Mutex<Option<CancellationToken>>);

impl Drop for ClearSlot<'_> {
    fn drop(&mut self) {
        *self.0.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
    }
}

/// The check, the download and the staging of one profile's updates (C# `AppUpdateService`).
pub struct Service {
    transport: Arc<dyn UpdateTransport>,
    updates_directory: PathBuf,
    public_key: Option<[u8; 32]>,
    downloading: Mutex<Option<CancellationToken>>,
    stall: Duration,
    package: UpdatePackage,
    needs_public_key: bool,
}

/// No folder: the profile's path names the user.
impl std::fmt::Debug for Service {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Service").field("verifies_signatures", &self.verifies_signatures()).finish_non_exhaustive()
    }
}

impl Service {
    /// The service for the profile whose data folder is `data_directory`, with the build's key (Windows rule 1: without one it never
    /// checks).
    pub fn new(transport: Arc<dyn UpdateTransport>, data_directory: &Path, public_key: Option<[u8; 32]>) -> Service {
        Service {
            transport,
            updates_directory: data_directory.join(UPDATES_FOLDER),
            public_key,
            downloading: Mutex::new(None),
            stall: DOWNLOAD_STALL,
            package: UpdatePackage::WindowsZip,
            needs_public_key: true,
        }
    }

    /// The same service with `profile`'s update rules (the package, the words without a key); [`Service::new`] has Windows'.
    pub fn with_profile(mut self, profile: &PlatformProfile) -> Service {
        self.package = profile.update_package;
        self.needs_public_key = profile.update_needs_public_key;
        self
    }

    /// The package's name in its version folder.
    pub fn package_name(&self) -> &'static str {
        package_name(self.package)
    }

    /// The same service with another stall time ([`DOWNLOAD_STALL`]; tests use a short one).
    pub fn with_stall_timeout(mut self, stall: Duration) -> Service {
        self.stall = stall;
        self
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
            return Err(no_public_key(self.needs_public_key).into());
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
        let destination = version_directory.join(self.package_name());
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
        let slot = ClearSlot(&self.downloading);
        let written = self.write_to_disk(url, &destination, asset.size, progress.clone(), &owned).await;
        drop(slot);
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
    /// the cap, with C#'s progress steps; a transfer with no byte for the stall time is stopped (`packageSize` with what arrived).
    async fn write_to_disk(
        &self,
        mut url: String,
        destination: &Path,
        expected: u64,
        progress: Arc<dyn Fn(f64) + Send + Sync>,
        cancel: &CancellationToken,
    ) -> Result<(), ServiceError> {
        let reported = Arc::new(Mutex::new(-1i64));
        let started = Instant::now();
        // The last time bytes arrived (milliseconds after `started`) and how many have.
        let active = Arc::new(AtomicU64::new(0));
        let received = Arc::new(AtomicU64::new(0));
        let on_bytes: mighty_platform::DownloadProgress = {
            let (reported, active, received) = (reported.clone(), active.clone(), received.clone());
            Arc::new(move |written: u64| {
                active.store(u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX), Ordering::SeqCst);
                received.store(written, Ordering::SeqCst);
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
                timeout: PACKAGE_TIMEOUT,
                progress: on_bytes.clone(),
                cancel: flag.clone(),
            };
            active.store(u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX), Ordering::SeqCst);
            let mut on_drop = CancelOnDrop { flag: flag.clone(), armed: true };
            let mut transfer = self.transport.download(request);
            let mut watch = tokio::time::interval((self.stall / 4).clamp(Duration::from_millis(5), Duration::from_secs(1)));
            // A cancel or a stall sets the transport's own flag and lets it end on its own, so its file is closed before the folder goes.
            let answer = loop {
                tokio::select! {
                    answer = &mut transfer => break answer,
                    () = cancel.cancelled() => {
                        flag.store(true, Ordering::SeqCst);
                        let _ = transfer.await;
                        return Err(ServiceError::Cancelled);
                    }
                    _ = watch.tick() => {
                        let since = Duration::from_millis(active.load(Ordering::SeqCst));
                        if started.elapsed().saturating_sub(since) >= self.stall {
                            flag.store(true, Ordering::SeqCst);
                            let _ = transfer.await;
                            return Err(UpdateError::Key {
                                key: "appUpdate.error.packageSize",
                                args: vec![("received", received.load(Ordering::SeqCst).to_string()), ("expected", expected.to_string())],
                            }
                            .into());
                        }
                    }
                }
            };
            on_drop.armed = false;
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
    /// `MightyCode.exe`; on the Mac exactly one `.app` at its top) an earlier run left under `updates`; None when there is none or the
    /// folder cannot be read.
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
                || !folder.join(self.package_name()).is_file()
                || !folder.join(STAGED_FOLDER).is_dir()
            {
                continue;
            }
            let whole = match self.package {
                UpdatePackage::WindowsZip => count_named(&folder.join(STAGED_FOLDER), super::EXECUTABLE_NAME).ok()?,
                UpdatePackage::MacBundle => super::stage_mac::bundles(&folder.join(STAGED_FOLDER)).len(),
            };
            if whole == 1 {
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
