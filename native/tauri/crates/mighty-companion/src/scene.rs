//! The pet's picture as a display list (C# WinUI\CompanionOverlay.cs's drawing; plan §0.6, D8): the sprite cell, the bubble's rectangle,
//! its text runs, buttons, pager and plan blocks, and the self-drawn menu, laid out by [`crate::layout`] and drawn by the platform's
//! `PetWindow`. The display list's types are mighty-platform's (`pet`), re-exported here, because the platform crates draw them and
//! depend on nothing else; this module builds them. A golden test compares each animation row's and each card kind's scene, as JSON,
//! with checked-in files (W8-E). Owner: W8-E.
//!
//! At rest (W8 Step 0): the types only.

pub use mighty_platform::pet::{PetBitmap, PetColor, PetItem, PetPoint, PetRect, PetScene, PetText, PetTextAlign, PetTextRun};
