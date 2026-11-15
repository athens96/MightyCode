//! The update's state and coordinator (C# `AppUpdateState`, `AppUpdateKeptCheck`, `AppUpdateCoordinator`, AppUpdateCoordinator.cs:43-463;
//! plan §1.4). Owner: W9-A; W9-C's host drives it on one task (C# uses one dispatcher) and draws its state.
//!
//! Phases `Idle…Failed`; `check` with the staged-version rules (the same version staged, or a newer one not for this architecture, keeps
//! Ready); `download` deferring while staging is busy; `install(relaunch)`; `install_on_quit`; `check_kept_update`;
//! `retry_deferred_staging`; `press`; `show_for_smoke`. The presentation is mighty-core's `app_update_view::describe` over
//! [`UpdateState::view`].

use std::path::PathBuf;

use chrono::{DateTime, Utc};

use super::manifest::Availability;
use super::plan::InstallPlan;
use super::{Phase, State};

/// What a look for a kept update at start found (C# `AppUpdateKeptCheck`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum KeptCheck {
    /// Nothing kept, or the switch off.
    #[default]
    None,
    /// A kept update that must wait (staging busy, a check or download running).
    Waiting,
    Checked,
}

/// What the app update box shows now (C# `AppUpdateState`); not saved.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct UpdateState {
    pub phase: Phase,
    pub download_fraction: f64,
    pub availability: Option<Availability>,
    pub package_path: Option<PathBuf>,
    pub staged_directory: Option<PathBuf>,
    pub install_plan: Option<InstallPlan>,
    pub error_message: Option<String>,
    pub checked_at: Option<DateTime<Utc>>,
    /// The last failure, kept beside Ready so a failed background check never takes a staged update away.
    pub last_error: Option<String>,
}

impl UpdateState {
    /// What `app_update_view::describe` reads of it.
    pub fn view(&self) -> State {
        State {
            phase: self.phase,
            version: self.availability.as_ref().map(|a| a.manifest.version.clone()),
            notes: self.availability.as_ref().and_then(|a| a.manifest.notes.clone()),
            download_fraction: self.download_fraction,
            checked_at: self.checked_at,
            error_message: self.error_message.clone(),
            last_error: self.last_error.clone(),
        }
    }
}

/// C# `AppUpdateCoordinator`: one profile's update from check to install. Step 0: the type and its state; the operations are W9-A's.
#[derive(Debug, Default)]
pub struct Coordinator {
    state: UpdateState,
    built_in_manifest_url: Option<String>,
}

impl Coordinator {
    /// A coordinator at rest, with the build's address (C#'s `builtInManifestUrl`).
    pub fn new(built_in_manifest_url: Option<String>) -> Coordinator {
        Coordinator { state: UpdateState::default(), built_in_manifest_url }
    }

    pub fn state(&self) -> &UpdateState {
        &self.state
    }

    /// C# `BuiltInManifestUrl`.
    pub fn built_in_manifest_url(&self) -> Option<&str> {
        self.built_in_manifest_url.as_deref()
    }
}
