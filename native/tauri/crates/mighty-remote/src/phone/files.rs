//! The phone's read-only view of a workspace folder (C# R\MobileWorkspaceFiles.cs, `MobilePreviewSlot`; macOS MobileWorkspaceFiles.swift):
//! the files pane's listing and preview (mighty-files) cut to what fits one relay frame. Containment is exactly the pane's — every path
//! goes through `workspace_files::resolve` and `open_file` — and a refusal carries a `code` beside its message, which the phone words
//! itself. The router reads the folder and the file two at a time and decodes one image at a time (`PreviewSlot`). Owner: W8-B.

use std::collections::VecDeque;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use mighty_core::json::JsonOut;
use mighty_core::locale::Locale;
use mighty_core::workspace_files;
use mighty_files::changes::ChangeSet;
use mighty_files::classify::{self, PreviewKind, SourceLanguage};
use mighty_files::listing::{self, FileEntry, ListError};
use mighty_files::{OpenError, OpenedFile, diff, open_file, source};
use mighty_platform::{FileHandles, PdfRaster};
use tokio::sync::oneshot;

use crate::phone::desk::MobileError;
use crate::phone::mighty::MobileOut;

pub const MAXIMUM_ENTRIES: usize = 2_000;
/// Text read from a file for the phone.
pub const MAXIMUM_TEXT_BYTES: usize = 512 * 1_024;
/// The encoded reply body: text and entries are cut further when escaping would take them over.
pub const MAXIMUM_REPLY_BYTES: usize = 768 * 1_024;
pub const MAXIMUM_THUMBNAIL_PIXELS: u32 = 2_048;
pub const MAXIMUM_THUMBNAIL_BYTES: usize = 512 * 1_024;
/// Long sides tried, largest first, until the thumbnail fits its bytes.
pub const THUMBNAIL_LADDER: [u32; 7] = [2_048, 1_600, 1_280, 1_024, 768, 512, 256];

fn refusal(status: u16, key: &str, code: &str, locale: &Locale) -> MobileError {
    MobileError::request(status, locale.get(key), Some(code))
}

/// `MobileWorkspaceFiles.WorkspaceNotFound` and the other refusals, each with its code.
pub fn workspace_not_found(l: &Locale) -> MobileError {
    refusal(404, "remote.error.workspaceNotFound", "workspaceNotFound", l)
}
pub fn not_found(l: &Locale) -> MobileError {
    refusal(404, "remote.files.notFound", "notFound", l)
}
pub fn outside_workspace(l: &Locale) -> MobileError {
    refusal(403, "remote.files.outsideWorkspace", "outsideWorkspace", l)
}
pub fn not_readable(l: &Locale) -> MobileError {
    refusal(403, "remote.files.notReadable", "notReadable", l)
}
pub fn not_directory(l: &Locale) -> MobileError {
    refusal(400, "phone.files.error.notDirectory", "notDirectory", l)
}
pub fn bad_path(l: &Locale) -> MobileError {
    refusal(400, "common.badPath", "badPath", l)
}
/// A newer preview from the same phone came in while this one waited for its turn.
pub fn superseded(l: &Locale) -> MobileError {
    refusal(409, "remote.files.superseded", "superseded", l)
}

/// `Stamp`: a time as `yyyy-MM-ddTHH:mm:ssZ` in UTC.
fn stamp(date: Option<SystemTime>) -> Option<String> {
    let since = date?.duration_since(SystemTime::UNIX_EPOCH).ok()?;
    let ticks = 621_355_968_000_000_000i64 + since.as_secs() as i64 * 10_000_000;
    let text = mighty_core::time::DateTimeOffset::from_utc_ticks(ticks).to_o();
    Some(format!("{}Z", &text[..19]))
}

/// `ValidatedPath(raw, allowRoot)`: "" (the root) only where allowed, no absolute path, no `..`, no empty or `.` component, no NUL, no
/// backslash, drive or stream name.
pub fn validated_path(raw: Option<&str>, allow_root: bool, l: &Locale) -> Result<String, MobileError> {
    let path = raw.unwrap_or("");
    if path.is_empty() {
        return if allow_root { Ok(String::new()) } else { Err(bad_path(l)) };
    }
    if path.contains('\0') || path.len() > workspace_files::MAXIMUM_PATH_BYTES {
        return Err(bad_path(l));
    }
    let components: Vec<&str> = path.split('/').collect();
    if path.starts_with('/') || path.contains('\\') || path.contains(':') || components.contains(&"..") {
        return Err(outside_workspace(l));
    }
    if components.iter().any(|c| c.is_empty() || *c == ".") {
        return Err(bad_path(l));
    }
    Ok(path.to_string())
}

/// `Refusal(relativePath, root)`: a path the resolver refused. Only an item directly inside a folder that resolves inside the root is
/// looked at (a link out of the root, or a dangling one, is outside the workspace); anything else is `notFound` alike.
pub fn refusal_for(relative: &str, root: &Path, l: &Locale) -> MobileError {
    let (parent, name) = match relative.rfind('/') {
        Some(at) => (&relative[..at], &relative[at + 1..]),
        None => ("", relative),
    };
    let Some(folder) = (!name.is_empty()).then(|| workspace_files::resolve(parent, root)).flatten() else { return not_found(l) };
    if std::fs::symlink_metadata(folder.join(name)).is_ok() { outside_workspace(l) } else { not_found(l) }
}

/// `MobileFileEntry(Name, RelativePath, Kind, Size, Modified, Noise)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MobileFileEntry {
    pub name: String,
    pub relative_path: String,
    pub kind: String,
    pub size: Option<u64>,
    pub modified: Option<String>,
    pub noise: bool,
}

impl MobileOut for MobileFileEntry {
    fn write(&self, out: &mut JsonOut) {
        out.begin_object();
        out.prop_str("name", Some(&self.name));
        out.prop_str("relativePath", Some(&self.relative_path));
        out.prop_str("kind", Some(&self.kind));
        out.prop_opt_int("size", self.size.map(|s| s as i64));
        out.prop_str("modified", self.modified.as_deref());
        out.prop_bool("noise", self.noise);
        out.end_object();
    }
}

/// `MobileFileListing(Protocol, WorkspaceId, Path, Entries, Truncated)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MobileFileListing {
    pub workspace_id: String,
    pub path: String,
    pub entries: Vec<MobileFileEntry>,
    pub truncated: bool,
}

impl MobileOut for MobileFileListing {
    fn write(&self, out: &mut JsonOut) {
        out.begin_object();
        out.prop_int("protocol", 1);
        out.prop_str("workspaceId", Some(&self.workspace_id));
        out.prop_str("path", Some(&self.path));
        out.name("entries");
        self.entries.write(out);
        out.prop_bool("truncated", self.truncated);
        out.end_object();
    }
}

/// `MobileFilePreview`: which optional fields are present follows `kind` — text for `source`/`markdown`, pixels for `image`, a `reason`
/// (`binary`, `notRegularFile`, `tooLarge`, `undecodable`) for `unsupported`; `changes` only with `changes=1` on text.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct MobileFilePreview {
    pub workspace_id: String,
    pub path: String,
    pub name: String,
    pub size: u64,
    pub modified: Option<String>,
    pub kind: String,
    pub language: Option<String>,
    pub encoding: Option<String>,
    pub text: Option<String>,
    pub truncated: Option<bool>,
    pub line_count: Option<usize>,
    pub mime: Option<String>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub thumbnail_width: Option<u32>,
    pub thumbnail_height: Option<u32>,
    pub data: Option<String>,
    pub reason: Option<String>,
    pub changes: Option<ChangeSet>,
}

impl MobileOut for MobileFilePreview {
    fn write(&self, out: &mut JsonOut) {
        out.begin_object();
        out.prop_int("protocol", 1);
        out.prop_str("workspaceId", Some(&self.workspace_id));
        out.prop_str("path", Some(&self.path));
        out.prop_str("name", Some(&self.name));
        out.prop_int("size", self.size as i64);
        out.prop_str("modified", self.modified.as_deref());
        out.prop_str("type", Some(&self.kind));
        out.prop_str("language", self.language.as_deref());
        out.prop_str("encoding", self.encoding.as_deref());
        out.prop_str("text", self.text.as_deref());
        out.prop_opt_bool("truncated", self.truncated);
        out.prop_opt_int("lineCount", self.line_count.map(|c| c as i64));
        out.prop_str("mime", self.mime.as_deref());
        out.prop_opt_int("width", self.width.map(i64::from));
        out.prop_opt_int("height", self.height.map(i64::from));
        out.prop_opt_int("thumbnailWidth", self.thumbnail_width.map(i64::from));
        out.prop_opt_int("thumbnailHeight", self.thumbnail_height.map(i64::from));
        out.prop_str("data", self.data.as_deref());
        out.prop_str("reason", self.reason.as_deref());
        if let Some(changes) = &self.changes {
            out.name("changes");
            out.value(&changes.to_json());
        }
        out.end_object();
    }
}

/// `Listing(workspaceId, path, root)`.
pub fn listing(workspace_id: &str, path: &str, root: &Path, l: &Locale) -> Result<MobileFileListing, MobileError> {
    let found = listing::list(path, root, listing::MAXIMUM_ENUMERATED_NAMES).map_err(|error| match error {
        ListError::OutsideRoot => refusal_for(path, root, l),
        ListError::NotDirectory => not_directory(l),
        ListError::Unreadable => not_readable(l),
    })?;
    let real_root = workspace_files::real_path(root);
    let truncated = found.truncated || found.entries.len() > MAXIMUM_ENTRIES;
    Ok(MobileFileListing {
        workspace_id: workspace_id.into(),
        path: path.into(),
        entries: found.entries.iter().take(MAXIMUM_ENTRIES).map(|e| entry(e, root, real_root.as_deref())).collect(),
        truncated,
    })
}

/// `Entry`: size and date are never read through a link — a link's are read at the real path the resolver checked against the root.
fn entry(entry: &FileEntry, root: &Path, real_root: Option<&Path>) -> MobileFileEntry {
    let kind = format!("{}{}", if entry.is_symlink { "symlink-" } else { "" }, if entry.is_directory { "folder" } else { "file" });
    let target = if entry.is_symlink {
        workspace_files::resolve(&entry.relative_path, root)
    } else {
        real_root.map(|r| r.join(entry.relative_path.replace('/', std::path::MAIN_SEPARATOR_STR)))
    };
    let base = MobileFileEntry {
        name: entry.name.clone(),
        relative_path: entry.relative_path.clone(),
        kind,
        size: None,
        modified: None,
        noise: entry.is_noise(),
    };
    if let Some(meta) = target.and_then(|t| std::fs::metadata(t).ok())
        && meta.is_dir() == entry.is_directory
    {
        return MobileFileEntry { size: (!entry.is_directory).then_some(meta.len()), modified: stamp(meta.modified().ok()), ..base };
    }
    base
}

fn bytes(value: &impl MobileOut) -> usize {
    value.to_json().len()
}

/// `Fitted(listing)`: under [`MAXIMUM_REPLY_BYTES`], dropping entries from the end (and saying so).
pub fn fitted_listing(mut listing: MobileFileListing) -> MobileFileListing {
    while bytes(&listing) > MAXIMUM_REPLY_BYTES && !listing.entries.is_empty() {
        let count = listing.entries.len();
        listing.entries.truncate(count - (count / 4).max(1));
        listing.truncated = true;
    }
    listing
}

/// `Fitted(preview)`: text and the changes' lines escaping would take over the limit are cut by a quarter at a time (marked truncated).
pub fn fitted_preview(mut preview: MobileFilePreview) -> MobileFilePreview {
    loop {
        let has_text = preview.text.as_ref().is_some_and(|t| !t.is_empty());
        let has_changes = preview.changes.as_ref().is_some_and(|c| !c.hunks.is_empty());
        if bytes(&preview) <= MAXIMUM_REPLY_BYTES || !(has_text || has_changes) {
            return preview;
        }
        if has_text && let Some(text) = &preview.text {
            let units: Vec<u16> = text.encode_utf16().collect();
            let mut cut = units.len() * 3 / 4;
            if cut > 0 && (0xD800..0xDC00).contains(&units[cut - 1]) {
                cut -= 1;
            }
            let kept = String::from_utf16_lossy(&units[..cut]);
            preview.line_count = Some(source::scan(&kept).0.len());
            preview.text = Some(kept);
            preview.truncated = Some(true);
        }
        if has_changes && let Some(changes) = &mut preview.changes {
            let lines: usize = changes.hunks.iter().map(|h| h.lines.len()).sum();
            changes.hunks = diff::cut(&changes.hunks, lines * 3 / 4);
            changes.truncated = true;
        }
    }
}

/// `SourceLanguage.ToString().ToLowerInvariant()`.
fn language_word(language: SourceLanguage) -> &'static str {
    match language {
        SourceLanguage::Swift => "swift",
        SourceLanguage::C => "c",
        SourceLanguage::JavaScript => "javascript",
        SourceLanguage::Python => "python",
        SourceLanguage::Kotlin => "kotlin",
        SourceLanguage::Java => "java",
        SourceLanguage::Go => "go",
        SourceLanguage::Rust => "rust",
        SourceLanguage::Shell => "shell",
        SourceLanguage::Json => "json",
        SourceLanguage::Yaml => "yaml",
        SourceLanguage::Toml => "toml",
        SourceLanguage::Xml => "xml",
        SourceLanguage::Html => "html",
        SourceLanguage::Css => "css",
        SourceLanguage::Sql => "sql",
        SourceLanguage::Gradle => "gradle",
        SourceLanguage::Dockerfile => "dockerfile",
        SourceLanguage::Makefile => "makefile",
        SourceLanguage::Plain => "plain",
    }
}

/// `MobileWorkspaceFiles.Stage`: a preview read as far as it goes without decoding an image; an image's file is left open, unread.
#[derive(Debug)]
pub struct Stage {
    pub preview: MobileFilePreview,
    pub image: Option<OpenedFile>,
}

/// `Prepare(workspaceId, path, root)`: opens and classifies the file; text is read here (up to 512 KiB), an image is not.
pub fn prepare(workspace_id: &str, path: &str, root: &Path, handles: &dyn FileHandles, l: &Locale) -> Result<Stage, MobileError> {
    let name = path.rsplit('/').next().unwrap_or("").to_string();
    let mut file = match open_file(path, root, handles) {
        Ok(file) => file,
        Err(OpenError::Missing) => return Err(refusal_for(path, root, l)),
        Err(OpenError::NotRegularFile) => {
            // A folder or a device: its details only, as the desktop shows it.
            let Some(real) = workspace_files::resolve(path, root) else { return Err(refusal_for(path, root, l)) };
            let (mut size, mut modified) = (0, None);
            if let Ok(meta) = std::fs::metadata(&real) {
                modified = stamp(meta.modified().ok());
                if !meta.is_dir() {
                    size = meta.len();
                }
            }
            let preview = MobileFilePreview {
                workspace_id: workspace_id.into(),
                path: path.into(),
                name,
                size,
                modified,
                kind: "unsupported".into(),
                reason: Some("notRegularFile".into()),
                ..MobileFilePreview::default()
            };
            return Ok(Stage { preview, image: None });
        }
        Err(OpenError::Unreadable) => return Err(not_readable(l)),
    };
    let mut preview = MobileFilePreview {
        workspace_id: workspace_id.into(),
        path: path.into(),
        name: name.clone(),
        size: file.size,
        modified: stamp(file.modified),
        kind: "unsupported".into(),
        ..MobileFilePreview::default()
    };
    let head = classify::read_head(&mut file.file).map_err(|_| not_readable(l))?;
    match classify::classify(&name, &head) {
        PreviewKind::Image => {
            if file.size > classify::MAXIMUM_IMAGE_BYTES {
                preview.reason = Some("tooLarge".into());
                return Ok(Stage { preview, image: None });
            }
            Ok(Stage { preview, image: Some(file) })
        }
        kind @ (PreviewKind::Markdown | PreviewKind::Source(_)) => {
            let read = classify::read_text(&mut file.file, MAXIMUM_TEXT_BYTES).map_err(|_| not_readable(l))?;
            preview.kind = kind.word().into();
            preview.language = match kind {
                PreviewKind::Source(language) => Some(language_word(language).into()),
                _ => None,
            };
            preview.encoding = Some(classify::display_name(read.encoding).into());
            preview.line_count = Some(source::scan(&read.text).0.len());
            preview.truncated = Some(read.truncated);
            preview.text = Some(read.text);
            Ok(Stage { preview, image: None })
        }
        PreviewKind::Unsupported => {
            preview.reason = Some("binary".into());
            Ok(Stage { preview, image: None })
        }
    }
}

/// `ImageBytes(file)`: the bytes of the image `prepare` left open; None when it grew past the image limit meanwhile.
pub fn image_bytes(file: &mut OpenedFile, l: &Locale) -> Result<Option<Vec<u8>>, MobileError> {
    let limit = classify::MAXIMUM_IMAGE_BYTES as usize;
    let data = classify::read_prefix(&mut file.file, limit + 1).map_err(|_| not_readable(l))?;
    Ok((data.len() <= limit).then_some(data))
}

// ── MobilePreviewSlot ───────────────────────────────────────────────────────────────────────────────────────────────────

#[derive(Default)]
struct SlotState {
    taken: bool,
    next: u64,
    waiters: VecDeque<(u64, String, oneshot::Sender<bool>)>,
}

/// `MobilePreviewSlot`: the image previews' own turn, one at a time; a newer preview from the same phone takes an older one's place in
/// the queue (the older one answers false, superseded).
#[derive(Clone, Default)]
pub struct PreviewSlot {
    state: Arc<Mutex<SlotState>>,
}

impl std::fmt::Debug for PreviewSlot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PreviewSlot").field("waiting", &self.waiting()).finish()
    }
}

/// A turn being waited for: dropped before it was answered (the phone gave up), it leaves the queue, and a turn it was given meanwhile
/// is handed on.
struct Turn {
    slot: PreviewSlot,
    id: u64,
    receiver: Option<oneshot::Receiver<bool>>,
}

impl Drop for Turn {
    fn drop(&mut self) {
        let Some(mut receiver) = self.receiver.take() else { return };
        let waiting = {
            let mut state = self.slot.lock();
            let before = state.waiters.len();
            state.waiters.retain(|(id, _, _)| *id != self.id);
            state.waiters.len() != before
        };
        if !waiting && matches!(receiver.try_recv(), Ok(true)) {
            self.slot.release();
        }
    }
}

impl PreviewSlot {
    pub fn new() -> PreviewSlot {
        PreviewSlot::default()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, SlotState> {
        self.state.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// `Waiting`.
    pub fn waiting(&self) -> usize {
        self.lock().waiters.len()
    }

    /// `Acquire(device)`: true when the caller holds the slot and must [`PreviewSlot::release`] it; false when a newer preview from the
    /// same phone replaced this one. Dropping the future gives the turn up.
    pub async fn acquire(&self, device: &str) -> bool {
        let mut turn = {
            let mut state = self.lock();
            if !state.taken {
                state.taken = true;
                return true;
            }
            if let Some(at) = state.waiters.iter().position(|(_, d, _)| d == device)
                && let Some((_, _, older)) = state.waiters.remove(at)
            {
                let _ = older.send(false);
            }
            let (sender, receiver) = oneshot::channel();
            state.next += 1;
            let id = state.next;
            state.waiters.push_back((id, device.to_string(), sender));
            Turn { slot: self.clone(), id, receiver: Some(receiver) }
        };
        let answer = match turn.receiver.as_mut() {
            Some(receiver) => receiver.await.unwrap_or(false),
            None => false,
        };
        turn.receiver = None;
        answer
    }

    /// `Release`: the next in line decodes, or the slot is free.
    pub fn release(&self) {
        loop {
            let next = {
                let mut state = self.lock();
                match state.waiters.pop_front() {
                    None => {
                        state.taken = false;
                        return;
                    }
                    Some(next) => next,
                }
            };
            // A waiter that gave up between its removal check and now takes nothing: the turn goes to the one after it.
            if next.2.send(true).is_ok() {
                return;
            }
        }
    }
}

// ── thumbnails (C# MainWindow.MobileImages.cs `MakeMobileImagePreview`) ────────────────────────────────────────────────────

/// The most pixels, and the longest side, a picture may have to be drawn for the phone.
pub const MAXIMUM_DECODE_PIXELS: u64 = 64_000_000;
pub const MAXIMUM_DECODE_SIDE: u32 = 32_768;

/// The most memory one picture's decode may take (`image::Limits::max_alloc`): a 64-megapixel RGBA picture is about 256 MiB.
pub const MAXIMUM_DECODE_ALLOC: u64 = 300 * 1024 * 1024;

/// mighty-files' `raster::decode` (the picture turned upright) with the decode's memory capped at [`MAXIMUM_DECODE_ALLOC`].
fn decode_capped(bytes: &[u8]) -> Option<image::DynamicImage> {
    use image::ImageDecoder as _;
    let mut reader = image::ImageReader::new(std::io::Cursor::new(bytes)).with_guessed_format().ok()?;
    let mut limits = image::Limits::default();
    limits.max_alloc = Some(MAXIMUM_DECODE_ALLOC);
    reader.limits(limits);
    let mut decoder = reader.into_decoder().ok()?;
    let orientation = decoder.orientation().unwrap_or(image::metadata::Orientation::NoTransforms);
    let mut picture = image::DynamicImage::from_decoder(decoder).ok()?;
    picture.apply_orientation(orientation);
    Some(picture)
}

fn undecodable() -> crate::phone::desk::MobileImageResult {
    crate::phone::desk::MobileImageResult { image: None, reason: Some("undecodable".into()) }
}

/// `MakeMobileImagePreview(name, bytes)`: an SVG drawn first (no larger than 2,048 px), a PDF's first page through the platform's PDF
/// port (at most twice its size, its long side 2,048 px), then the thumbnail ladder: the long side steps down from 2,048 px until the
/// picture fits 512 KiB — a JPEG at quality 82 when every pixel is opaque, else a PNG. `tooLarge` past 64 megapixels or 32,768 px a
/// side; `undecodable` for anything that does not draw.
pub fn thumbnail(name: &str, bytes: &[u8], pdf: &dyn PdfRaster) -> crate::phone::desk::MobileImageResult {
    use image::ImageEncoder as _;
    if bytes.is_empty() {
        return undecodable();
    }
    let extension = classify::file_extension(name);
    let drawn: Vec<u8> = match extension.as_str() {
        "svg" => match mighty_files::raster::svg_png(bytes, MAXIMUM_THUMBNAIL_PIXELS, false) {
            Ok(raster) => raster.png,
            Err(_) => return undecodable(),
        },
        "pdf" => {
            let size = |width: f64, height: f64| {
                if !(width.is_finite() && height.is_finite() && width > 0.0 && height > 0.0) {
                    return None;
                }
                let scale = (MAXIMUM_THUMBNAIL_PIXELS as f64 / width.max(height)).min(2.0);
                Some(((width * scale).max(1.0) as u32, (height * scale).max(1.0) as u32))
            };
            match pdf.first_page(bytes, &size) {
                Ok(Some(page)) => match page.png {
                    Some(png) => png,
                    None => return undecodable(),
                },
                _ => return undecodable(),
            }
        }
        _ => bytes.to_vec(),
    };
    let Some(size) = mighty_files::raster::bitmap_size(&drawn) else { return undecodable() };
    let (width, height) = (size.width, size.height);
    if width == 0 || height == 0 {
        return undecodable();
    }
    if (width as u64) * (height as u64) > MAXIMUM_DECODE_PIXELS || width > MAXIMUM_DECODE_SIDE || height > MAXIMUM_DECODE_SIDE {
        return crate::phone::desk::MobileImageResult { image: None, reason: Some("tooLarge".into()) };
    }
    let Some(picture) = decode_capped(&drawn) else { return undecodable() };
    let rgba = picture.to_rgba8();
    let opaque = rgba.pixels().all(|p| p.0[3] == 255);
    for edge in THUMBNAIL_LADDER {
        let scale = (edge as f64 / width.max(height) as f64).min(1.0);
        let target_width = ((width as f64 * scale).round() as u32).max(1);
        let target_height = ((height as f64 * scale).round() as u32).max(1);
        let scaled = if (target_width, target_height) == (width, height) {
            rgba.clone()
        } else {
            image::imageops::resize(&rgba, target_width, target_height, image::imageops::FilterType::Triangle)
        };
        let mut out = Vec::new();
        let written = if opaque {
            let rgb = image::DynamicImage::ImageRgba8(scaled).to_rgb8();
            image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 82).write_image(
                rgb.as_raw(),
                target_width,
                target_height,
                image::ExtendedColorType::Rgb8,
            )
        } else {
            image::codecs::png::PngEncoder::new(&mut out).write_image(
                scaled.as_raw(),
                target_width,
                target_height,
                image::ExtendedColorType::Rgba8,
            )
        };
        if written.is_err() {
            return undecodable();
        }
        if out.len() > MAXIMUM_THUMBNAIL_BYTES {
            continue;
        }
        use base64::Engine as _;
        return crate::phone::desk::MobileImageResult {
            image: Some(crate::phone::desk::MobileImagePreview {
                mime: if opaque { "image/jpeg" } else { "image/png" }.into(),
                data: base64::engine::general_purpose::STANDARD.encode(&out),
                width,
                height,
                thumbnail_width: target_width,
                thumbnail_height: target_height,
            }),
            reason: None,
        };
    }
    crate::phone::desk::MobileImageResult { image: None, reason: Some("tooLarge".into()) }
}
