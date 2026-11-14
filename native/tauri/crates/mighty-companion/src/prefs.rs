//! `<profile>\companion-settings.json` (`Wire.Json`, nulls left out): `enabled=true, showsTask=true, notifications=true,
//! reducedMotion=false, selectedPet="mighty-raccoon", bubbleWidth? (220-640), bubbleHeight? (100-480), left?, top?`; read at most 64 KiB
//! through a safe path with one hard link (any error: the defaults), written to a `.companion-<guid>` temporary file then moved (no flush),
//! unknown keys dropped. Written only with `--allow-state-write` (the smoke: its own profile) (C# Core\Companion.cs `CompanionPreferences`;
//! plan §1.7). Owner: W8-E.
//!
//! At rest (W8 Step 0): nothing is read or written: [`load`] gives the defaults and [`save`] refuses.

use std::path::Path;

/// The file in the profile.
pub const FILE: &str = "companion-settings.json";

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

/// `CompanionPreferences.Load(profile)`: the profile's file, or the defaults on any error.
pub fn load(profile: &Path) -> CompanionPreferences {
    let _ = profile;
    CompanionPreferences::default()
}

/// `CompanionPreferences.Save(profile)` (the caller checks it may write).
pub fn save(profile: &Path, preferences: &CompanionPreferences) -> std::io::Result<()> {
    let _ = (profile, preferences);
    Err(std::io::Error::new(std::io::ErrorKind::Unsupported, "the companion's settings are not written yet (W8-E)"))
}
