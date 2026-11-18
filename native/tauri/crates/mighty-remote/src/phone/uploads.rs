//! The phone's uploads (C# R\MobileUploads.cs; plan §1.5): bounded and device-owned — chunks of 196,608 bytes in order, 5 MiB a file,
//! 8 MiB a submit, 64 open and 16 per session, abandoned ones swept after 10 minutes — staged in `<dir>/<id>.part` files held open by
//! the store (the stale ones of an earlier run deleted at start), and a submit spends its claim only after the desktop accepted the
//! request. Owner: W8-B.

use std::collections::HashMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use mighty_core::attachment::RunAttachment;
use mighty_core::json::JsonOut;
use mighty_run::attachments;

use crate::phone::desk::MobileError;
use crate::phone::mighty::MobileOut;

/// `MobileUploads.ChunkSize`.
pub const CHUNK_SIZE: usize = 196_608;
/// How long an upload nobody claimed is kept after its last chunk.
pub const EXPIRY: Duration = Duration::from_secs(10 * 60);
pub const MAXIMUM_OPEN: usize = 64;
pub const MAXIMUM_PER_SESSION: usize = 16;

/// `MobileUploadTicket(Protocol, UploadId, ChunkSize)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MobileUploadTicket {
    pub upload_id: String,
}

impl MobileOut for MobileUploadTicket {
    fn write(&self, out: &mut JsonOut) {
        out.begin_object();
        out.prop_int("protocol", 1);
        out.prop_str("uploadId", Some(&self.upload_id));
        out.prop_int("chunkSize", CHUNK_SIZE as i64);
        out.end_object();
    }
}

/// `MobileUploadAttachment(Id, Name, Size)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MobileUploadAttachment {
    pub id: String,
    pub name: String,
    pub size: usize,
}

impl MobileOut for MobileUploadAttachment {
    fn write(&self, out: &mut JsonOut) {
        out.begin_object();
        out.prop_str("id", Some(&self.id));
        out.prop_str("name", Some(&self.name));
        out.prop_int("size", self.size as i64);
        out.end_object();
    }
}

/// `MobileUploadClaim(Id, UploadIds, Attachments)`: the files a submit takes, spent by [`MobileUploads::finish`].
#[derive(Debug, Clone, PartialEq)]
pub struct MobileUploadClaim {
    pub id: String,
    pub upload_ids: Vec<String>,
    pub attachments: Vec<RunAttachment>,
}

struct Upload {
    session: String,
    device: String,
    name: String,
    size: usize,
    path: PathBuf,
    file: File,
    received: usize,
    next: i64,
    completed: bool,
    claim: Option<String>,
    updated: Instant,
}

impl Drop for Upload {
    /// C#'s `FileOptions.DeleteOnClose`: the staged bytes go with the handle.
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

fn not_found() -> MobileError {
    MobileError::request(404, "Upload not found.", None)
}

fn io(_: std::io::Error) -> MobileError {
    MobileError::Unavailable
}

/// Unicode's Format characters (general category Cf), which an upload's name drops with the controls.
fn format_character(c: char) -> bool {
    matches!(c as u32,
        0xAD | 0x600..=0x605 | 0x61C | 0x6DD | 0x70F | 0x890..=0x891 | 0x8E2 | 0x180E | 0x200B..=0x200F | 0x202A..=0x202E
        | 0x2060..=0x2064 | 0x2066..=0x206F | 0xFEFF | 0xFFF9..=0xFFFB | 0x110BD | 0x110CD | 0x13430..=0x1343F | 0x1BCA0..=0x1BCA3
        | 0x1D173..=0x1D17A | 0xE0001 | 0xE0020..=0xE007F)
}

/// Whether a stem reads as a `Guid` (`N` or `D` form), as an upload's file is named.
fn guid(stem: &str) -> bool {
    let hex = |s: &str| s.bytes().all(|b| b.is_ascii_hexdigit());
    match stem.len() {
        32 => hex(stem),
        36 => {
            let parts: Vec<&str> = stem.split('-').collect();
            parts.iter().map(|p| p.len()).eq([8, 4, 4, 4, 12]) && parts.iter().all(|p| hex(p))
        }
        _ => false,
    }
}

/// `MobileUploads`: bounded, device-owned uploads.
pub struct MobileUploads {
    directory: PathBuf,
    uploads: Mutex<HashMap<String, Upload>>,
    /// The folder was made and its stale files swept.
    ready: std::sync::atomic::AtomicBool,
}

/// Whether `folder` is a real folder: not a link, a junction or any other reparse point.
fn plain_folder(folder: &Path) -> bool {
    let Ok(meta) = std::fs::symlink_metadata(folder) else { return false };
    let plain = meta.is_dir() && !meta.file_type().is_symlink();
    #[cfg(windows)]
    let plain = plain && {
        use std::os::windows::fs::MetadataExt as _;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
        meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT == 0
    };
    plain
}

/// An upload's staging file, made new: on Windows shared with nobody and deleted when its handle closes (C#'s `FileShare.None`,
/// `FileOptions.DeleteOnClose`), elsewhere readable and writable by its owner alone (0600).
fn staging_file(path: &Path) -> std::io::Result<File> {
    let mut options = std::fs::OpenOptions::new();
    options.read(true).write(true).create_new(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt as _;
        const FILE_FLAG_DELETE_ON_CLOSE: u32 = 0x0400_0000;
        options.share_mode(0).custom_flags(FILE_FLAG_DELETE_ON_CLOSE);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    options.open(path)
}

impl std::fmt::Debug for MobileUploads {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MobileUploads").field("count", &self.lock().len()).finish()
    }
}

impl MobileUploads {
    /// The store in `directory`. Nothing is made or read until the first upload begins ([`MobileUploads::begin`]): a router that never
    /// takes an upload (a read-only launch, a smoke) leaves its folder alone.
    pub fn new(directory: &Path) -> MobileUploads {
        MobileUploads {
            directory: directory.to_path_buf(),
            uploads: Mutex::new(HashMap::new()),
            ready: std::sync::atomic::AtomicBool::new(false),
        }
    }

    /// The folder made on first use, owner-only where the OS says so (Unix 0700), and the `.part` files an earlier run left there deleted
    /// (at most 1,024). A folder that is a link, a junction or another reparse point is refused: no upload is written through it.
    fn prepare(&self) -> Result<(), MobileError> {
        if self.ready.load(std::sync::atomic::Ordering::SeqCst) {
            return Ok(());
        }
        std::fs::create_dir_all(&self.directory).map_err(io)?;
        if !plain_folder(&self.directory) {
            return Err(MobileError::Unavailable);
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let _ = std::fs::set_permissions(&self.directory, std::fs::Permissions::from_mode(0o700));
        }
        if let Ok(entries) = std::fs::read_dir(&self.directory) {
            for entry in entries.flatten().take(1_024) {
                let path = entry.path();
                let stale = path.extension().is_some_and(|e| e == "part")
                    && path.file_stem().and_then(|s| s.to_str()).is_some_and(guid)
                    && entry.file_type().is_ok_and(|t| t.is_file());
                if stale {
                    let _ = std::fs::remove_file(&path);
                }
            }
        }
        self.ready.store(true, std::sync::atomic::Ordering::SeqCst);
        Ok(())
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, Upload>> {
        self.uploads.lock().unwrap_or_else(|p| p.into_inner())
    }

    fn sweep(uploads: &mut HashMap<String, Upload>) {
        uploads.retain(|_, u| u.claim.is_some() || u.updated.elapsed() <= EXPIRY);
    }

    fn owned<'a>(uploads: &'a mut HashMap<String, Upload>, id: &str, device: &str) -> Result<&'a mut Upload, MobileError> {
        uploads.get_mut(id).filter(|u| u.device == device).ok_or_else(not_found)
    }

    /// `Count`: the uploads this computer holds for the phone (abandoned ones swept first).
    pub fn count(&self) -> usize {
        let mut uploads = self.lock();
        Self::sweep(&mut uploads);
        uploads.len()
    }

    /// `Begin(session, device, rawName, size)`.
    pub fn begin(&self, session: &str, device: &str, raw_name: &str, size: i64) -> Result<MobileUploadTicket, MobileError> {
        self.prepare()?;
        let mut uploads = self.lock();
        Self::sweep(&mut uploads);
        let replaced = raw_name.replace('\\', "/");
        let component = replaced.rsplit('/').next().unwrap_or("");
        let kept: String = component.chars().filter(|c| !c.is_control() && !format_character(*c)).collect();
        let name: String = kept.trim_start_matches('.').trim().chars().take(120).collect();
        let units = name.encode_utf16().count();
        if units == 0 || units > 180 || size <= 0 {
            return Err(MobileError::request(400, "Invalid attachment name or size.", None));
        }
        if size > attachments::MAXIMUM_FILE_BYTES as i64 {
            return Err(MobileError::request(413, "Attachment exceeds 5 MiB.", None));
        }
        if uploads.len() >= MAXIMUM_OPEN || uploads.values().filter(|u| u.session == session).count() >= MAXIMUM_PER_SESSION {
            return Err(MobileError::request(429, "Too many open uploads.", None));
        }
        let id = mighty_core::wire::new_id();
        let path = self.directory.join(format!("{id}.part"));
        let file = staging_file(&path).map_err(io)?;
        uploads.insert(
            id.clone(),
            Upload {
                session: session.into(),
                device: device.into(),
                name,
                size: size as usize,
                path,
                file,
                received: 0,
                next: 0,
                completed: false,
                claim: None,
                updated: Instant::now(),
            },
        );
        Ok(MobileUploadTicket { upload_id: id })
    }

    /// `Append(id, device, index, data)`: the bytes received so far.
    pub fn append(&self, id: &str, device: &str, index: i64, data: &[u8]) -> Result<usize, MobileError> {
        let mut uploads = self.lock();
        Self::sweep(&mut uploads);
        let u = Self::owned(&mut uploads, id, device)?;
        if u.completed || u.claim.is_some() || index != u.next {
            return Err(MobileError::request(409, "Chunks must arrive once, in order.", None));
        }
        if data.len() != CHUNK_SIZE.min(u.size - u.received) || data.is_empty() {
            return Err(MobileError::request(400, "Chunk size does not match declaration.", None));
        }
        u.file.seek(SeekFrom::Start(u.received as u64)).map_err(io)?;
        u.file.write_all(data).map_err(io)?;
        u.received += data.len();
        u.next += 1;
        u.updated = Instant::now();
        Ok(u.received)
    }

    /// `Complete(id, device)`.
    pub fn complete(&self, id: &str, device: &str) -> Result<MobileUploadAttachment, MobileError> {
        let mut uploads = self.lock();
        Self::sweep(&mut uploads);
        let u = Self::owned(&mut uploads, id, device)?;
        if u.completed {
            return Err(MobileError::request(409, "Upload already completed.", None));
        }
        if u.received != u.size {
            return Err(MobileError::request(400, "Upload is incomplete.", None));
        }
        u.completed = true;
        u.updated = Instant::now();
        Ok(MobileUploadAttachment { id: id.into(), name: u.name.clone(), size: u.size })
    }

    /// `Claim(ids, session, device)`: the completed uploads a submit takes, read into attachments; none of them can be claimed again
    /// until [`MobileUploads::finish`].
    pub fn claim(&self, ids: &[String], session: &str, device: &str) -> Result<MobileUploadClaim, MobileError> {
        let mut uploads = self.lock();
        Self::sweep(&mut uploads);
        let distinct: std::collections::HashSet<&String> = ids.iter().collect();
        if ids.len() > attachments::MAXIMUM_COUNT || distinct.len() != ids.len() {
            return Err(MobileError::request(400, "Invalid attachment list.", None));
        }
        for id in ids {
            Self::owned(&mut uploads, id, device)?;
        }
        if ids.iter().any(|id| uploads.get(id).is_some_and(|u| u.session != session || !u.completed || u.claim.is_some())) {
            return Err(MobileError::request(400, "Attachment is unavailable for this session.", None));
        }
        if ids.iter().filter_map(|id| uploads.get(id)).map(|u| u.size).sum::<usize>() > attachments::MAXIMUM_TOTAL_BYTES {
            return Err(MobileError::request(413, "Attachments exceed 8 MiB.", None));
        }
        let mut files = vec![];
        for id in ids {
            let Some(u) = uploads.get_mut(id) else { return Err(not_found()) };
            u.file.seek(SeekFrom::Start(0)).map_err(io)?;
            let mut bytes = vec![0; u.size];
            u.file.read_exact(&mut bytes).map_err(io)?;
            files.push(attachments::make(&u.name, &bytes).map_err(|_| MobileError::Invalid)?);
        }
        let claim = mighty_core::wire::new_id();
        for id in ids {
            if let Some(u) = uploads.get_mut(id) {
                u.claim = Some(claim.clone());
            }
        }
        Ok(MobileUploadClaim { id: claim, upload_ids: ids.to_vec(), attachments: files })
    }

    /// `Finish(claim, accepted)`: an accepted submit spends its uploads (their bytes deleted); one that was not leaves them available.
    pub fn finish(&self, claim: &MobileUploadClaim, accepted: bool) {
        let mut uploads = self.lock();
        for id in &claim.upload_ids {
            if uploads.get(id).is_some_and(|u| u.claim.as_deref() == Some(claim.id.as_str())) {
                if accepted {
                    uploads.remove(id);
                } else if let Some(u) = uploads.get_mut(id) {
                    u.claim = None;
                }
            }
        }
    }

    /// `Cancel(id, device)`.
    pub fn cancel(&self, id: &str, device: &str) -> Result<(), MobileError> {
        let mut uploads = self.lock();
        let u = Self::owned(&mut uploads, id, device)?;
        if u.claim.is_some() {
            return Err(MobileError::request(409, "Upload is being submitted.", None));
        }
        uploads.remove(id);
        Ok(())
    }

    /// `DiscardSession(session)`: a closed pane's uploads go with it.
    pub fn discard_session(&self, session: &str) {
        self.lock().retain(|_, u| u.session != session);
    }

    /// `Clear`.
    pub fn clear(&self) {
        self.lock().clear();
    }
}

impl Drop for MobileUploads {
    fn drop(&mut self) {
        self.clear();
    }
}
