//! The companion pet in the window (C# WinUI/MainWindow.Companion.cs, MainWindow.CompanionControls.cs, MainWindow.WebShell.Settings.cs:
//! 138-139, 318-336, 462-470) over mighty-companion and the platform's `PetWindow`. Owner: W8-E.
//!
//! W8 Step 0 wrote the seams main.rs, methods.rs and host.rs call, each at rest (no pet, no status bar controls, the page methods doing
//! nothing), so none of them needs a further W8 change. What W8-E fills here (W8 plan, W8-E): `companion-settings.json` read (written only
//! with `--allow-state-write`), the pet window opened through the main window's `SurfaceFactory::open_pet` only with the flag (else the box
//! says verification mode; a smoke opens only its own test overlay: NOACTIVATE, focus checked, closed in `finally`), the 1/12 s timer,
//! following `claude` panes (permission, activity and awaiting-notice overrides), the card key guard (SHA-256 of tool and input), actions
//! answered through the run service, review/questions/plan opening the page's own card by focusing the pane (D10), `companionStatus`
//! (the `companion` popover NativeNode surface, `mighty_native::View::Companion`), `companionToggle`, and the frame model's controls.

#![allow(dead_code, reason = "W8-E's seams, at rest until it fills them")]

use std::sync::Arc;

use mighty_core::shell::CompanionFrame;
use mighty_platform::{Platform, SurfaceFactory};
use serde_json::Value;

use crate::host::{Host, Reply};

/// The companion's start with the app, once the main window's native views exist (its pet window is one). At rest: nothing.
pub fn start(host: &Arc<Host>, platform: &Platform, surfaces: Arc<dyn SurfaceFactory>) {
    let _ = (host, platform, surfaces);
}

/// The app quits: the pet window closed and the timer stopped. At rest: nothing.
pub fn shutdown(host: &Host) {
    let _ = host;
}

/// `companionStatus` (the status bar's capsule): true when it opened the `companion` popover. At rest: false (nothing opens; the
/// capsule is not drawn while [`frame_model`] gives none).
pub async fn status(host: Arc<Host>, args: Value) -> Reply {
    let _ = (host, args);
    Ok(Value::Bool(false))
}

/// `companionToggle` (the status bar's paw): `enabled` flipped, saved, the pet shown or hidden. Answers null. At rest: nothing.
pub async fn toggle(host: Arc<Host>, args: Value) -> Reply {
    let _ = (host, args);
    Ok(Value::Null)
}

/// C# `WebCompanionControls` for the frame model, read with the state locked (never `host.view()`). At rest: none, so the page draws
/// neither the paw nor the capsule, as before W8.
pub fn frame_model(host: &Host) -> Option<CompanionFrame> {
    let _ = host;
    None
}
