//! The pets: the bundled `assets\pets\mighty-raccoon`, then `<profile>\pets\*` (`local:`), then `$CODEX_HOME|~\.codex\pets\*` (`codex:`),
//! 40 per folder, dot-folders skipped; an atlas is PNG or WebP, at most 20 MiB, 1536 wide, 1872/2288/2496 tall (versions 1-3), alpha read
//! from the headers before any decode, cells 192×208, 9 rows of `[6,8,8,4,5,8,6,6,6]` frames (C# Core\Companion.cs `CompanionPet`; plan
//! §1.7). The full decode (C#'s `DecodeCompanionPet` and `CompanionPreview`, WinUI) is here too, with the `image` crate: premultiplied
//! BGRA, its size the header's, at least one pixel not opaque. Owner: W8-E.

use std::path::{Path, PathBuf};

use mighty_core::json::JsonOut;
use mighty_platform::pet::PetBitmap;
use serde_json::Value;

use crate::prefs::read_bounded;

/// One atlas cell's size in pixels.
pub const CELL_WIDTH: u32 = 192;
pub const CELL_HEIGHT: u32 = 208;
/// The frames of each of the nine rows (C# `CompanionPet.FrameCounts`).
pub const ROW_FRAMES: [u32; 9] = [6, 8, 8, 4, 5, 8, 6, 6, 6];
/// The bundled pet.
pub const DEFAULT_PET: &str = "mighty-raccoon";
/// C# `CompanionPet.MaximumBytes`.
pub const MAXIMUM_BYTES: usize = 20 * 1024 * 1024;
/// The atlas's width, and its heights by sprite version (1, 2, 3).
pub const ATLAS_WIDTH: u32 = 1536;
pub const ATLAS_HEIGHTS: [u32; 3] = [1872, 2288, 2496];
/// Folders read in each pets folder.
pub const MAXIMUM_FOLDERS: usize = 40;

const DURATIONS: [&[f64]; 9] = [
    &[0.28, 0.11, 0.11, 0.14, 0.14, 0.32],
    &[0.12, 0.12, 0.12, 0.12, 0.12, 0.12, 0.12, 0.22],
    &[0.12, 0.12, 0.12, 0.12, 0.12, 0.12, 0.12, 0.22],
    &[0.14, 0.14, 0.14, 0.28],
    &[0.14, 0.14, 0.14, 0.14, 0.28],
    &[0.14, 0.14, 0.14, 0.14, 0.14, 0.14, 0.14, 0.24],
    &[0.15, 0.15, 0.15, 0.15, 0.15, 0.26],
    &[0.12, 0.12, 0.12, 0.12, 0.12, 0.22],
    &[0.15, 0.15, 0.15, 0.15, 0.15, 0.28],
];

/// `CompanionPet.Frame(row, elapsed, reducedMotion)`: the frame of `row` `elapsed` seconds into its loop; 0 under reduced motion.
pub fn frame(row: i32, elapsed: f64, reduced_motion: bool) -> u32 {
    if reduced_motion || !elapsed.is_finite() {
        return 0;
    }
    let timing = DURATIONS[row.clamp(0, 8) as usize];
    let total: f64 = timing.iter().sum();
    let mut phase = elapsed.max(0.0) % total;
    for (index, duration) in timing.iter().enumerate() {
        if phase < *duration {
            return index as u32;
        }
        phase -= duration;
    }
    0
}

/// C# `CompanionPet(Id, Name, Source, Width, Height, Image)`: a pet whose atlas passed the header checks (`image` empty in a catalog).
#[derive(Clone, PartialEq, Eq)]
pub struct CompanionPet {
    pub id: String,
    pub name: String,
    pub source: PathBuf,
    pub width: u32,
    pub height: u32,
    pub image: Vec<u8>,
}

impl std::fmt::Debug for CompanionPet {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CompanionPet")
            .field("id", &self.id)
            .field("name", &self.name)
            .field("source", &self.source)
            .field("width", &self.width)
            .field("height", &self.height)
            .field("image", &self.image.len())
            .finish()
    }
}

/// `CompanionPet.Invalid(reason)`: the error every refusal reads as.
pub fn invalid(reason: &str) -> String {
    format!("Invalid Codex pet: {reason}. Expected transparent PNG/WebP, 1536 × 1872/2288/2496, at most 20 MiB.")
}

/// `StyleManifestDecoder.SafeRelativePath`.
pub fn safe_relative_path(path: &str) -> bool {
    !path.trim().is_empty()
        && !path.starts_with('/')
        && !path.starts_with('~')
        && !path.starts_with('\\')
        && !path.contains(':')
        && !path.split(['/', '\\']).any(|part| part == "..")
}

fn split(path: &Path) -> Option<(PathBuf, String)> {
    Some((path.parent()?.to_path_buf(), path.file_name()?.to_string_lossy().into_owned()))
}

fn extension(path: &Path) -> String {
    path.extension().map(|e| format!(".{}", e.to_string_lossy().to_lowercase())).unwrap_or_default()
}

/// `Path.GetFullPath`.
fn full(path: &Path) -> PathBuf {
    mighty_core::workspace_files::full_path(path).unwrap_or_else(|| path.to_path_buf())
}

impl CompanionPet {
    /// `CompanionPet.Load(selected, id)`: a folder with its `pet.json`, a manifest, or a sprite sheet itself.
    pub fn load(selected: &Path, id: &str) -> Result<CompanionPet, String> {
        let selected = full(selected);
        let manifest = if selected.is_dir() { selected.join("pet.json") } else { selected.clone() };
        let mut source = selected.clone();
        let mut name = selected.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        let mut version: Option<i32> = None;
        if extension(&manifest) == ".json" {
            let (folder, file) = split(&manifest).ok_or_else(|| invalid("manifest"))?;
            let bytes = read_bounded(&folder, &file, 65_536).ok_or_else(|| invalid("manifest"))?;
            let value: Value = serde_json::from_slice(&bytes).map_err(|_| invalid("manifest"))?;
            let object = value.as_object().ok_or_else(|| invalid("manifest path"))?;
            let path = object
                .get("spritesheetPath")
                .and_then(Value::as_str)
                .filter(|p| safe_relative_path(p))
                .ok_or_else(|| invalid("manifest path"))?;
            source = mighty_core::workspace_files::resolve(path, &folder).ok_or_else(|| invalid("manifest path"))?;
            if let Some(title) = object.get("displayName").and_then(Value::as_str) {
                name = mighty_core::wire::clean(title, 80);
            }
            if let Some(v) = object.get("spriteVersionNumber") {
                version = Some(v.as_i64().and_then(|n| i32::try_from(n).ok()).ok_or_else(|| invalid("version"))?);
            }
        }
        if !matches!(extension(&source).as_str(), ".png" | ".webp") {
            return Err(invalid("format"));
        }
        let (folder, file) = split(&source).ok_or_else(|| invalid("image"))?;
        let image = read_bounded(&folder, &file, MAXIMUM_BYTES as u64).ok_or_else(|| invalid("image"))?;
        let (width, height, alpha) = inspect_image(&image)?;
        if width != ATLAS_WIDTH || !ATLAS_HEIGHTS.contains(&height) || !alpha {
            return Err(invalid("dimensions or transparency"));
        }
        if let Some(version) = version {
            let expected = match version {
                1 => 1872,
                2 => 2288,
                3 => 2496,
                _ => 0,
            };
            if expected != height {
                return Err(invalid("version"));
            }
        }
        let name = if name.trim().is_empty() { "Pet".to_string() } else { name };
        Ok(CompanionPet { id: id.into(), name, source, width, height, image })
    }

    /// `CompanionPet.Catalog(bundled, profile, codexHome)`: the bundled pet, then the profile's, then Codex's, each without its atlas.
    pub fn catalog(bundled: &Path, profile: &Path, codex_home: Option<&Path>) -> Vec<CompanionPet> {
        let mut result = Vec::new();
        let mut add = |path: &Path, id: String| {
            if let Ok(pet) = CompanionPet::load(path, &id) {
                result.push(CompanionPet { image: vec![], ..pet });
            }
        };
        add(&bundled.join(DEFAULT_PET), DEFAULT_PET.into());
        let codex = codex_home.map(Path::to_path_buf).unwrap_or_else(|| match std::env::var_os("CODEX_HOME") {
            Some(home) => PathBuf::from(home),
            None => home_folder().join(".codex"),
        });
        for (root, prefix) in [(profile.join("pets"), "local:"), (codex.join("pets"), "codex:")] {
            let Ok(entries) = std::fs::read_dir(&root) else { continue };
            let mut folders: Vec<(String, PathBuf)> = entries
                .flatten()
                .filter(|e| e.path().is_dir())
                .map(|e| (e.file_name().to_string_lossy().into_owned(), e.path()))
                .filter(|(name, _)| !name.starts_with('.'))
                .collect();
            folders.sort_by(|a, b| a.0.cmp(&b.0));
            for (name, path) in folders.into_iter().take(MAXIMUM_FOLDERS) {
                add(&path, format!("{prefix}{name}"));
            }
        }
        result
    }

    /// `CompanionPet.Install(profile)`: the validated bytes (never the source again) written to `<profile>\pets\<guid>` with a
    /// `pet.json` naming them, and that folder loaded as `local:<guid>`. The caller fully decodes and checks the image first.
    pub fn install(&self, profile: &Path) -> Result<CompanionPet, String> {
        let io = |e: std::io::Error| e.to_string();
        let root = profile.join("pets");
        std::fs::create_dir_all(&root).map_err(io)?;
        let real_profile = mighty_core::workspace_files::real_path(profile);
        let real_root = mighty_core::workspace_files::real_path(&root);
        let linked = std::fs::symlink_metadata(&root).map(|m| m.file_type().is_symlink()).unwrap_or(true)
            || matches!(mighty_core::workspace_files::link_target(&root), Ok(Some(_)));
        let (Some(real_profile), Some(real_root)) = (real_profile, real_root) else { return Err(invalid("destination")) };
        if !mighty_core::workspace_files::contains(&real_root, &real_profile) || linked {
            return Err(invalid("destination"));
        }
        let id = uuid::Uuid::new_v4().simple().to_string();
        let destination = real_root.join(&id);
        std::fs::create_dir_all(&destination).map_err(io)?;
        let written = (|| -> Result<CompanionPet, String> {
            let filename = format!("spritesheet{}", extension(&self.source));
            let create = |name: &str, bytes: &[u8]| -> Result<(), String> {
                use std::io::Write;
                let mut file = std::fs::OpenOptions::new().write(true).create_new(true).open(destination.join(name)).map_err(io)?;
                file.write_all(bytes).map_err(io)
            };
            create(&filename, &self.image)?;
            let mut manifest = JsonOut::new();
            manifest.begin_object();
            manifest.prop_str("id", Some(&id));
            manifest.prop_str("displayName", Some(&self.name));
            manifest.prop_str("spritesheetPath", Some(&filename));
            manifest.end_object();
            create("pet.json", manifest.finish().as_bytes())?;
            CompanionPet::load(&destination, &format!("local:{id}"))
        })();
        if written.is_err() {
            let _ = std::fs::remove_dir_all(&destination);
        }
        written
    }

    /// C# `DecodeCompanionPet` (WinUI): the whole atlas decoded as premultiplied BGRA, its size the header's and at least one pixel not
    /// opaque, named by the pet's id and its bytes' length (a platform keeps its copy while the key stays).
    pub fn decode(&self) -> Result<PetBitmap, String> {
        let failed = || mighty_run::strings::get("companion.error.decode");
        let decoded = image::load_from_memory(&self.image).map_err(|_| failed())?;
        if decoded.width() != self.width || decoded.height() != self.height {
            return Err(failed());
        }
        let rgba = decoded.to_rgba8();
        let mut bgra = Vec::with_capacity(rgba.len());
        let mut clear = false;
        for pixel in rgba.pixels() {
            let [r, g, b, a] = pixel.0;
            clear |= a < 255;
            let premultiply = |c: u8| ((u32::from(c) * u32::from(a) + 127) / 255) as u8;
            bgra.extend_from_slice(&[premultiply(b), premultiply(g), premultiply(r), a]);
        }
        if !clear {
            return Err(failed());
        }
        Ok(PetBitmap {
            key: format!("{}@{}", self.id, self.image.len()),
            width: self.width,
            height: self.height,
            bgra: std::sync::Arc::new(bgra),
        })
    }
}

/// `CompanionPreview` (WinUI): the atlas's first cell, 192 × 208, as a PNG (the settings' preview).
pub fn preview_png(atlas: &PetBitmap) -> Result<Vec<u8>, String> {
    if atlas.width < CELL_WIDTH || atlas.height < CELL_HEIGHT {
        return Err(mighty_run::strings::get("companion.error.decode"));
    }
    png(CELL_WIDTH, CELL_HEIGHT, atlas.width * 4, &atlas.bgra)
}

/// Premultiplied BGRA rows (`stride` bytes apart) as a straight-alpha PNG (C#'s `BitmapEncoder` over `BitmapAlphaMode.Premultiplied`):
/// the settings' preview and the smoke's captures.
pub fn png(width: u32, height: u32, stride: u32, bgra: &[u8]) -> Result<Vec<u8>, String> {
    let mut rgba = Vec::with_capacity((width * height * 4) as usize);
    for y in 0..height {
        let row = (y * stride) as usize;
        for x in 0..width {
            let at = row + (x * 4) as usize;
            let Some(&[b, g, r, a]) = bgra.get(at..at + 4).map(|p| <&[u8; 4]>::try_from(p).expect("four bytes")) else {
                return Err("the picture is smaller than its size".into());
            };
            // The premultiplied colour taken back.
            let straight = |c: u8| if a == 0 { 0 } else { ((u32::from(c) * 255 + u32::from(a) / 2) / u32::from(a)).min(255) as u8 };
            rgba.extend_from_slice(&[straight(r), straight(g), straight(b), a]);
        }
    }
    let mut out = Vec::new();
    image::codecs::png::PngEncoder::new(&mut out)
        .write_image(&rgba, width, height, image::ExtendedColorType::Rgba8)
        .map_err(|e| e.to_string())?;
    Ok(out)
}

/// A PNG's size (the smoke reads its preview back).
pub fn png_size(png: &[u8]) -> Option<(u32, u32)> {
    let decoded = image::load_from_memory_with_format(png, image::ImageFormat::Png).ok()?;
    Some((decoded.width(), decoded.height()))
}

use image::ImageEncoder;

fn home_folder() -> PathBuf {
    std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME")).map(PathBuf::from).unwrap_or_default()
}

fn be32(bytes: &[u8], at: usize) -> u32 {
    u32::from_be_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]])
}

fn le32(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]])
}

/// `CompanionPet.InspectImage`: the width, height and whether it has alpha, from the PNG or WebP headers alone (bounds checked before
/// any decoder runs).
pub fn inspect_image(image: &[u8]) -> Result<(u32, u32, bool), String> {
    if image.len() < 25 || image.len() > MAXIMUM_BYTES {
        return Err(invalid("size"));
    }
    if image[..8] == [137, 80, 78, 71, 13, 10, 26, 10] {
        if image.len() < 33 || &image[12..16] != b"IHDR" || be32(image, 8) != 13 {
            return Err(invalid("PNG header"));
        }
        let (width, height) = (be32(image, 16), be32(image, 20));
        if width > i32::MAX as u32 || height > i32::MAX as u32 {
            return Err(invalid("PNG header"));
        }
        let mut alpha = matches!(image[25], 4 | 6);
        let mut offset = 8usize;
        while offset + 12 <= image.len() {
            let length = be32(image, offset) as usize;
            if length > image.len() - offset - 12 {
                return Err(invalid("PNG chunk"));
            }
            if &image[offset + 4..offset + 8] == b"tRNS" {
                alpha = true;
            }
            offset += length + 12;
        }
        return Ok((width, height, alpha));
    }
    if &image[..4] != b"RIFF" || &image[8..12] != b"WEBP" || le32(image, 4) as usize != image.len() - 8 {
        return Err(invalid("WebP header"));
    }
    let mut offset = 12usize;
    while offset + 8 <= image.len() {
        let length = le32(image, offset + 4) as usize;
        if length > image.len() - offset - 8 {
            return Err(invalid("WebP chunk"));
        }
        let data = &image[offset + 8..offset + 8 + length];
        let tag = &image[offset..offset + 4];
        if tag == b"VP8X" && data.len() >= 10 {
            let width = 1 + u32::from(data[4]) + (u32::from(data[5]) << 8) + (u32::from(data[6]) << 16);
            let height = 1 + u32::from(data[7]) + (u32::from(data[8]) << 8) + (u32::from(data[9]) << 16);
            return Ok((width, height, data[0] & 16 != 0));
        }
        if tag == b"VP8L" && data.len() >= 5 && data[0] == 0x2f {
            let bits = le32(data, 1);
            return Ok(((bits & 0x3fff) + 1, ((bits >> 14) & 0x3fff) + 1, bits & (1 << 28) != 0));
        }
        offset += length + 8 + (length & 1);
    }
    Err(invalid("WebP transparency"))
}
