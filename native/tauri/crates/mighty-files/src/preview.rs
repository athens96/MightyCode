//! One file read for the preview, off the page's thread (C# `FilePreviewLoader.Load`, `FilePreviewData`; WorkspaceFiles.cs:986-1075):
//! classified from its first bytes; text read up to 1 MB in the encoding found; Markdown over 128 KB marked as source only; pictures
//! and SVG kept as bytes under 50 MB, an SVG refused when it can load outside content.

use std::path::Path;
use std::time::SystemTime;

use mighty_core::locale::Locale;
use mighty_platform::FileHandles;

use crate::classify::{self, PreviewKind, TextEncodingKind};
use crate::{OpenError, open_file};

/// C# `FilePreviewData`.
#[derive(Debug, Clone, PartialEq)]
pub struct PreviewData {
    pub name: String,
    pub relative_path: String,
    pub size: u64,
    pub modified: Option<SystemTime>,
    pub kind: PreviewKind,
    pub text: Option<String>,
    pub encoding: Option<TextEncodingKind>,
    pub disk_hash: Option<String>,
    pub truncated: bool,
    pub markdown_renderable: bool,
    pub image_bytes: Option<Vec<u8>>,
    pub reason: Option<String>,
    /// The preview could not be read at all: the "missing" or "failed" message.
    pub failure: Option<String>,
}

impl PreviewData {
    fn new(name: &str, relative_path: &str, size: u64, modified: Option<SystemTime>, kind: PreviewKind) -> PreviewData {
        PreviewData {
            name: name.into(),
            relative_path: relative_path.into(),
            size,
            modified,
            kind,
            text: None,
            encoding: None,
            disk_hash: None,
            truncated: false,
            markdown_renderable: false,
            image_bytes: None,
            reason: None,
            failure: None,
        }
    }
}

/// C# `FilePreviewLoader.Load`.
pub fn load(root: &Path, relative: &str, handles: &dyn FileHandles, locale: &Locale) -> PreviewData {
    let name = relative.rsplit('/').next().unwrap_or("").to_string();
    let mut opened = match open_file(relative, root, handles) {
        Ok(opened) => opened,
        Err(error) => {
            let mut data = PreviewData::new(&name, relative, 0, None, PreviewKind::Unsupported);
            data.failure = match error {
                OpenError::Missing => Some(locale.get("files.preview.missing")),
                OpenError::NotRegularFile => None,
                OpenError::Unreadable => Some(locale.get("files.preview.failed")),
            };
            return data;
        }
    };
    let (size, modified) = (opened.size, opened.modified);
    match read(&mut opened.file, &name, relative, size, modified, locale) {
        Ok(data) => data,
        Err(_) => PreviewData {
            failure: Some(locale.get("files.preview.failed")),
            ..PreviewData::new(&name, relative, size, modified, PreviewKind::Unsupported)
        },
    }
}

/// MC-33: a file read on an SSH machine (its first bytes, up to the remote read's cap) classified and shown as [`load`] shows a local
/// one. `longer` says the file went on past what was read: its text is marked cut, a picture is too large to show. No disk hash: the
/// remote preview is read-only.
pub fn from_bytes(relative: &str, bytes: Vec<u8>, longer: bool, locale: &Locale) -> PreviewData {
    let name = relative.rsplit('/').next().unwrap_or("").to_string();
    let size = bytes.len() as u64;
    let mut cursor = std::io::Cursor::new(bytes);
    let read = (|| -> std::io::Result<PreviewData> {
        let kind = classify::classify(&name, &classify::read_head(&mut cursor)?);
        Ok(match kind {
            PreviewKind::Image => {
                let mut data = PreviewData::new(&name, relative, size, None, PreviewKind::Unsupported);
                if longer {
                    data.reason = Some(locale.get("files.preview.tooLargeImage"));
                    return Ok(data);
                }
                let bytes = classify::read_prefix(&mut cursor, classify::MAXIMUM_IMAGE_BYTES as usize)?;
                if classify::file_extension(&name) == "svg" && classify::svg_loads_external_content(&bytes) {
                    data.reason = Some(locale.get("files.preview.svgExternal"));
                    return Ok(data);
                }
                PreviewData { image_bytes: Some(bytes), ..PreviewData::new(&name, relative, size, None, kind) }
            }
            PreviewKind::Source(_) | PreviewKind::Markdown => {
                let text = classify::read_text(&mut cursor, classify::MAXIMUM_TEXT_BYTES)?;
                PreviewData {
                    text: Some(text.text),
                    encoding: Some(text.encoding),
                    truncated: text.truncated || longer,
                    markdown_renderable: kind == PreviewKind::Markdown && !longer && size <= classify::MAXIMUM_MARKDOWN_RENDER_BYTES,
                    ..PreviewData::new(&name, relative, size, None, kind)
                }
            }
            PreviewKind::Unsupported => PreviewData::new(&name, relative, size, None, PreviewKind::Unsupported),
        })
    })();
    read.unwrap_or_else(|_| PreviewData {
        failure: Some(locale.get("files.preview.failed")),
        ..PreviewData::new(&name, relative, size, None, PreviewKind::Unsupported)
    })
}

/// The opened file classified and read.
fn read(
    file: &mut std::fs::File,
    name: &str,
    relative: &str,
    size: u64,
    modified: Option<SystemTime>,
    locale: &Locale,
) -> std::io::Result<PreviewData> {
    let kind = classify::classify(name, &classify::read_head(file)?);
    Ok(match kind {
        PreviewKind::Image => {
            if size > classify::MAXIMUM_IMAGE_BYTES {
                let mut data = PreviewData::new(name, relative, size, modified, PreviewKind::Unsupported);
                data.reason = Some(locale.get("files.preview.tooLargeImage"));
                return Ok(data);
            }
            let bytes = classify::read_prefix(file, classify::MAXIMUM_IMAGE_BYTES as usize)?;
            if classify::file_extension(name) == "svg" && classify::svg_loads_external_content(&bytes) {
                let mut data = PreviewData::new(name, relative, size, modified, PreviewKind::Unsupported);
                data.reason = Some(locale.get("files.preview.svgExternal"));
                return Ok(data);
            }
            PreviewData { image_bytes: Some(bytes), ..PreviewData::new(name, relative, size, modified, kind) }
        }
        PreviewKind::Source(_) | PreviewKind::Markdown => {
            let text = classify::read_text(file, classify::MAXIMUM_TEXT_BYTES)?;
            let disk_hash = if !text.truncated && size <= classify::MAXIMUM_TEXT_BYTES as u64 {
                classify::read_prefix(file, classify::MAXIMUM_TEXT_BYTES).ok().map(|raw| crate::edit::disk_hash(&raw))
            } else {
                None
            };
            PreviewData {
                text: Some(text.text),
                encoding: Some(text.encoding),
                disk_hash,
                truncated: text.truncated,
                markdown_renderable: kind == PreviewKind::Markdown && size <= classify::MAXIMUM_MARKDOWN_RENDER_BYTES,
                ..PreviewData::new(name, relative, size, modified, kind)
            }
        }
        PreviewKind::Unsupported => PreviewData::new(name, relative, size, modified, PreviewKind::Unsupported),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// MC-33: a remote file's bytes are shown as a local file's: text decoded, a cut file marked, a picture past the cap refused.
    #[test]
    fn remote_bytes_are_classified_like_a_local_file() {
        let locale = Locale::new("en");
        let text = from_bytes("src/main.rs", b"fn main() {}\n".to_vec(), false, &locale);
        assert!(matches!(text.kind, PreviewKind::Source(_)));
        assert_eq!(text.text.as_deref(), Some("fn main() {}\n"));
        assert!(!text.truncated && text.disk_hash.is_none() && text.name == "main.rs");
        let cut = from_bytes("README.md", b"# Title\n".to_vec(), true, &locale);
        assert!(cut.truncated && !cut.markdown_renderable);
        let picture = from_bytes("a.png", b"\x89PNG\r\n\x1a\n0000".to_vec(), true, &locale);
        assert_eq!(picture.kind, PreviewKind::Unsupported);
        assert!(picture.reason.is_some());
    }
}
