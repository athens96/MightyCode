//! M-F: the companion pet as an `NSPanel` `[.borderless, .nonactivatingPanel]`, clear, `level=.floating`, `becomesKeyOnlyIfNeeded`,
//! `hidesOnDeactivate=false`, `[.canJoinAllSpaces, .fullScreenAuxiliary]`, at the visible frame's bottom right, frame autosave
//! `MightyCodeCompanion`, drawing mighty-companion's `PetScene` with CoreGraphics and CoreText, click-through per hit. Swift
//! `AgentCompanionViews.swift:600-667`, `CompanionPet*.swift`. Contract `mighty_platform::PetWindow`.

use mighty_platform::{PetWindow, PlatformError, Result};

/// Opens the pet's panel (Unsupported until M-F).
pub async fn open(_window: &tauri::Window) -> Result<Box<dyn PetWindow>> {
    Err(PlatformError::unsupported("SurfaceFactory", "open_pet"))
}
