//! The desktop app's update as the phone sees it and starts it (C# Core\PhoneAppUpdate.cs; docs/phone-desktop-update.md; M:
//! MobileAppUpdate): the state's phase in the phone's words with the requests an install would stop (`/m1/state`'s `appUpdate`), and the
//! steps of the phone's install (`POST /m1/app-update`). Owner: W8-B.
//!
//! The update's own state is mighty-core's `app_update_view::State`, whose `version` and `notes` are the newer manifest's (C#'s
//! `Availability is { IsNewer: true }`): a state without a version offers nothing.

use mighty_core::app_update_view::{Phase, State};
use mighty_core::json::JsonOut;

use crate::phone::mighty::MobileOut;

/// `MobileAppUpdate(Phase, Current, Available?, Notes?, Progress?, Error?, Running)`.
#[derive(Debug, Clone, PartialEq)]
pub struct MobileAppUpdate {
    /// idle, checking, upToDate, available, downloading, ready, installing or failed.
    pub phase: String,
    pub current: String,
    pub available: Option<String>,
    pub notes: Option<String>,
    pub progress: Option<f64>,
    pub error: Option<String>,
    /// The requests an install stops.
    pub running: i32,
}

impl MobileAppUpdate {
    /// `MobileAppUpdate.MaximumNotes`.
    pub const MAXIMUM_NOTES: usize = 2_000;

    /// The record with its notes cut to [`MobileAppUpdate::MAXIMUM_NOTES`] UTF-16 units (C#'s `Notes` initializer).
    pub fn new(
        phase: &str,
        current: &str,
        available: Option<String>,
        notes: Option<String>,
        progress: Option<f64>,
        error: Option<String>,
        running: i32,
    ) -> MobileAppUpdate {
        let notes = notes.map(|text| {
            let units: Vec<u16> = text.encode_utf16().collect();
            if units.len() > Self::MAXIMUM_NOTES { String::from_utf16_lossy(&units[..Self::MAXIMUM_NOTES]) } else { text }
        });
        MobileAppUpdate { phase: phase.into(), current: current.into(), available, notes, progress, error, running }
    }
}

impl MobileOut for MobileAppUpdate {
    fn write(&self, out: &mut JsonOut) {
        out.begin_object();
        out.prop_str("phase", Some(&self.phase));
        out.prop_str("current", Some(&self.current));
        out.prop_str("available", self.available.as_deref());
        out.prop_str("notes", self.notes.as_deref());
        out.prop_opt_float("progress", self.progress);
        out.prop_str("error", self.error.as_deref());
        out.prop_int("running", self.running as i64);
        out.end_object();
    }
}

/// `PhoneAppUpdateStep`: what the phone's install does next as the update moves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PhoneAppUpdateStep {
    /// A step is under way: wait for the next change of the update.
    Wait,
    /// The request is over: nothing newer, or the update failed.
    Done,
    /// Look for an update first.
    Check,
    /// A newer version was found: download it.
    Download,
    /// The update is ready: install it and restart the app.
    Install,
}

/// `PhoneAppUpdate.Map`: the state for the phone; None when this build cannot update itself.
pub fn map(state: Option<&State>, current: &str, running: i32) -> Option<MobileAppUpdate> {
    let state = state?;
    let (phase, progress) = match state.phase {
        Phase::Checking => ("checking", None),
        Phase::UpToDate => ("upToDate", None),
        Phase::Available => ("available", None),
        Phase::Downloading => ("downloading", Some((state.download_fraction.clamp(0.0, 1.0) * 100.0).round_ties_even() / 100.0)),
        Phase::Staging => ("downloading", Some(1.0)),
        Phase::Ready => ("ready", None),
        Phase::Installing => ("installing", None),
        Phase::Failed => ("failed", None),
        Phase::Idle => ("idle", None),
    };
    let offered = state.version.is_some();
    Some(MobileAppUpdate::new(
        phase,
        current,
        state.version.clone(),
        state.notes.clone().filter(|_| offered),
        progress,
        if state.phase == Phase::Failed { state.error_message.clone() } else { None },
        running,
    ))
}

/// `PhoneAppUpdate.Begin`: what the app knows may be old, so it looks again unless an update is already under way.
pub fn begin(state: &State) -> PhoneAppUpdateStep {
    match state.phase {
        Phase::Idle | Phase::UpToDate | Phase::Failed => PhoneAppUpdateStep::Check,
        _ => next(state),
    }
}

/// `PhoneAppUpdate.Next`: the next step, asked at each change of the update while the phone's install is pending.
pub fn next(state: &State) -> PhoneAppUpdateStep {
    match state.phase {
        Phase::Available if state.version.is_some() => PhoneAppUpdateStep::Download,
        Phase::Available => PhoneAppUpdateStep::Done,
        Phase::Ready => PhoneAppUpdateStep::Install,
        Phase::Idle | Phase::UpToDate | Phase::Failed => PhoneAppUpdateStep::Done,
        _ => PhoneAppUpdateStep::Wait,
    }
}
