//! `<profile>\companion-settings.json` (`Wire.Json`, nulls left out): `enabled=true, showsTask=true, notifications=true,
//! reducedMotion=false, selectedPet="mighty-raccoon", bubbleWidth? (220-640), bubbleHeight? (100-480), left?, top?`; read at most 64 KiB
//! through a safe path (any error: the defaults), written to a `.companion-<guid>` temporary file then moved (no flush), unknown keys
//! dropped. Written only with `--allow-state-write` (the smoke: its own profile) (C# Core\Companion.cs `CompanionPreferences`; plan §1.7).
//! Owner: W8-E.
//!
//! The file is the C# app's, byte for byte: the same names in the same order, numbers as .NET writes them (`258`, not `258.0`) and
//! strings escaped as `Wire.Json` escapes them ([`mighty_core::json::JsonOut`]). `left` and `top` are the window's top left in the
//! desktop's physical pixels, as C# saves them, so either app puts the pet where the other left it.
//!
//! Reads go through `WorkspaceFiles.OpenFile` (mighty-composer `open_file`: resolved under the folder, no link at its end, the open
//! handle's final path checked again where the OS can ask) and, where the OS can ask a handle, refuse a file with more than one hard
//! link (C# `StyleFiles.SingleLink`, the platform's `FileHandles::single_link`).

use std::io::{Read, Write};
use std::path::Path;

use mighty_core::json::{self, JsonOut, Obj};
use serde_json::Value;

use crate::layout;

/// The file in the profile.
pub const FILE: &str = "companion-settings.json";
/// The most it reads (C# `StyleFiles.Read(profile, FILE, 65_536)`).
pub const MAXIMUM_BYTES: u64 = 65_536;

/// C# `CompanionPreferences(Enabled, ShowsTask, Notifications, ReducedMotion, SelectedPet, BubbleWidth?, BubbleHeight?, Left?, Top?)`.
#[derive(Debug, Clone, PartialEq)]
pub struct CompanionPreferences {
    pub enabled: bool,
    pub shows_task: bool,
    pub notifications: bool,
    pub reduced_motion: bool,
    pub selected_pet: String,
    pub bubble_width: Option<f64>,
    pub bubble_height: Option<f64>,
    pub left: Option<i32>,
    pub top: Option<i32>,
}

impl Default for CompanionPreferences {
    fn default() -> Self {
        CompanionPreferences {
            enabled: true,
            shows_task: true,
            notifications: true,
            reduced_motion: false,
            selected_pet: crate::pet::DEFAULT_PET.into(),
            bubble_width: None,
            bubble_height: None,
            left: None,
            top: None,
        }
    }
}

impl CompanionPreferences {
    /// `JsonSerializer.Serialize(this, Wire.Json)`: the record's properties in their order, a null left out.
    pub fn to_json(&self) -> String {
        let mut out = JsonOut::new();
        out.begin_object();
        out.prop_bool("enabled", self.enabled);
        out.prop_bool("showsTask", self.shows_task);
        out.prop_bool("notifications", self.notifications);
        out.prop_bool("reducedMotion", self.reduced_motion);
        out.prop_str("selectedPet", Some(&self.selected_pet));
        out.prop_opt_float("bubbleWidth", self.bubble_width);
        out.prop_opt_float("bubbleHeight", self.bubble_height);
        out.prop_opt_int("left", self.left.map(i64::from));
        out.prop_opt_int("top", self.top.map(i64::from));
        out.end_object();
        out.finish()
    }

    /// `JsonSerializer.Deserialize<CompanionPreferences>(bytes, Wire.Json)` with `Load`'s clamps: names matched without regard to case,
    /// a number read from a string too, a missing property its default, any other token or a value out of range an error.
    pub fn from_json(bytes: &[u8]) -> Option<CompanionPreferences> {
        let value: Value = serde_json::from_slice(bytes).ok()?;
        if value.is_null() {
            return Some(CompanionPreferences::default());
        }
        let object = Obj::of(&value, "CompanionPreferences").ok()?;
        let defaults = CompanionPreferences::default();
        let selected = match object.get("selectedPet") {
            // A null name is the record's null; written back it is left out, and read again it is the default.
            Some(Value::Null) => defaults.selected_pet.clone(),
            other => json::string(other, || Some(defaults.selected_pet.clone())).ok()?.unwrap_or_default(),
        };
        let read = CompanionPreferences {
            enabled: json::boolean(object.get("enabled"), defaults.enabled).ok()?,
            shows_task: json::boolean(object.get("showsTask"), defaults.shows_task).ok()?,
            notifications: json::boolean(object.get("notifications"), defaults.notifications).ok()?,
            reduced_motion: json::boolean(object.get("reducedMotion"), defaults.reduced_motion).ok()?,
            selected_pet: selected,
            bubble_width: json::opt_float64(object.get("bubbleWidth")).ok()?,
            bubble_height: json::opt_float64(object.get("bubbleHeight")).ok()?,
            left: json::opt_int32(object.get("left")).ok()?,
            top: json::opt_int32(object.get("top")).ok()?,
        };
        Some(CompanionPreferences {
            bubble_width: read.bubble_width.filter(|w| w.is_finite()).map(|w| layout::width(Some(w))),
            bubble_height: layout::height(read.bubble_height),
            ..read
        })
    }
}

/// C# `StyleFiles.Read(root, relative, maximum)`: the file at `relative` under `root`, resolved there, not a link at its end, at most
/// `maximum` bytes; None when any of that fails.
pub fn read_bounded(root: &Path, relative: &str, maximum: u64) -> Option<Vec<u8>> {
    let safe = !relative.trim().is_empty()
        && !relative.starts_with(['/', '~', '\\'])
        && !relative.contains(':')
        && !relative.split(['/', '\\']).any(|part| part == "..");
    if !safe || root.to_string_lossy().starts_with("\\\\") {
        return None;
    }
    let handles = mighty_composer::files::file_handles();
    let opened = mighty_composer::files::open_file(relative, root, handles.as_ref()).ok()?;
    if handles.asks_handles() && !handles.single_link(&opened.file) {
        return None;
    }
    if !opened.file.metadata().ok()?.is_file() || opened.size > maximum {
        return None;
    }
    let mut bytes = Vec::new();
    opened.file.take(maximum + 1).read_to_end(&mut bytes).ok()?;
    (bytes.len() as u64 <= maximum).then_some(bytes)
}

/// `CompanionPreferences.Load(profile)`: the profile's file, or the defaults on any error.
pub fn load(profile: &Path) -> CompanionPreferences {
    match read_bounded(profile, FILE, MAXIMUM_BYTES) {
        None => CompanionPreferences::default(),
        Some(bytes) => CompanionPreferences::from_json(&bytes).unwrap_or_default(),
    }
}

/// `CompanionPreferences.Save(profile)` (the caller checks it may write): a temporary file of its own made new, then moved over the
/// file, which may not be a link.
pub fn save(profile: &Path, preferences: &CompanionPreferences) -> std::io::Result<()> {
    std::fs::create_dir_all(profile)?;
    let destination = profile.join(FILE);
    if let Ok(meta) = std::fs::symlink_metadata(&destination)
        && meta.file_type().is_symlink()
    {
        return Err(std::io::Error::other("Companion settings cannot be a link."));
    }
    if matches!(mighty_core::workspace_files::link_target(&destination), Ok(Some(_))) {
        return Err(std::io::Error::other("Companion settings cannot be a link."));
    }
    let temp = profile.join(format!(".companion-{}", uuid::Uuid::new_v4().simple()));
    let written = (|| {
        let mut file = std::fs::OpenOptions::new().write(true).create_new(true).open(&temp)?;
        file.write_all(preferences.to_json().as_bytes())?;
        drop(file);
        std::fs::rename(&temp, &destination)
    })();
    let _ = std::fs::remove_file(&temp);
    written
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_file_is_written_as_the_csharp_app_writes_it() {
        assert_eq!(
            CompanionPreferences::default().to_json(),
            r#"{"enabled":true,"showsTask":true,"notifications":true,"reducedMotion":false,"selectedPet":"mighty-raccoon"}"#
        );
        let moved = CompanionPreferences {
            reduced_motion: true,
            selected_pet: "local:펫<\"x\">".into(),
            bubble_width: Some(318.0),
            bubble_height: Some(260.5),
            left: Some(-100),
            top: Some(200),
            ..CompanionPreferences::default()
        };
        assert_eq!(
            moved.to_json(),
            r#"{"enabled":true,"showsTask":true,"notifications":true,"reducedMotion":true,"selectedPet":"local:\uD3AB\u003C\u0022x\u0022\u003E","bubbleWidth":318,"bubbleHeight":260.5,"left":-100,"top":200}"#
        );
        assert_eq!(CompanionPreferences::from_json(moved.to_json().as_bytes()), Some(moved));
    }

    #[test]
    fn a_read_takes_what_wire_json_takes_and_clamps_the_bubble() {
        let read =
            CompanionPreferences::from_json(br#"{"ENABLED":false,"left":"12","bubbleWidth":9000,"bubbleHeight":1,"extra":[1]}"#).unwrap();
        assert_eq!((read.enabled, read.left, read.bubble_width, read.bubble_height), (false, Some(12), Some(640.0), Some(100.0)));
        assert_eq!(CompanionPreferences::from_json(b"null"), Some(CompanionPreferences::default()));
        for broken in [&br#"{"enabled":"yes"}"#[..], br#"{"left":1.5}"#, br#"{"top":null,"showsTask":null}"#, b"[", b"7"] {
            assert_eq!(CompanionPreferences::from_json(broken), None, "{}", String::from_utf8_lossy(broken));
        }
        assert_eq!(CompanionPreferences::from_json(br#"{"selectedPet":null}"#).unwrap().selected_pet, "mighty-raccoon");
    }
}
