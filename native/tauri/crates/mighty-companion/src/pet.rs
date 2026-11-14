//! The pets: the bundled `assets\pets\mighty-raccoon`, then `<profile>\pets\*` (`local:`), then `$CODEX_HOME|~\.codex\pets\*` (`codex:`),
//! 40 per folder, dot-folders skipped; an atlas is PNG or WebP, at most 20 MiB, 1536 wide, 1872/2288/2496 tall (versions 1-3), alpha read
//! from the headers before any decode, cells 192×208, 9 rows of `[6,8,8,4,5,8,6,6,6]` frames (C# Core\Companion.cs `CompanionPetCatalog`,
//! `CompanionAtlas`; plan §1.7). Owner: W8-E.
//!
//! At rest (W8 Step 0): the cell size and frame counts only.

/// One atlas cell's size in pixels.
pub const CELL_WIDTH: u32 = 192;
pub const CELL_HEIGHT: u32 = 208;
/// The frames of each of the nine rows.
pub const ROW_FRAMES: [u32; 9] = [6, 8, 8, 4, 5, 8, 6, 6, 6];
/// The bundled pet.
pub const DEFAULT_PET: &str = "mighty-raccoon";
