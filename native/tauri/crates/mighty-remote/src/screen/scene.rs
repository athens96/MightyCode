//! The reference scene's phase timing and frozen frame (C# R\ScreenShareReferenceScene.cs; plan §1.8): the same deterministic 3/60/30
//! second measurement fixture as macOS. Its document and terminal are synthetic; no application receives input. The window that draws
//! it (C# ScreenShareReferenceSceneWindow.cs) comes after 0.5 with the rest of the native screen share (D11). Owner: W8-F.

pub const PREROLL_SECONDS: f64 = 3.0;
pub const MOTION_SECONDS: f64 = 60.0;
pub const STILL_SECONDS: f64 = 30.0;
pub const TOTAL_SECONDS: f64 = 93.0;
pub const SCROLL_POINTS_PER_SECOND: f64 = 90.0;
pub const TYPED_CHARACTERS_PER_SECOND: f64 = 18.0;

/// C# `Moment(Phase, SecondsLeft, MotionElapsed)`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Moment {
    pub phase: &'static str,
    pub seconds_left: i32,
    pub motion_elapsed: f64,
}

/// `At`: the phase `elapsed` seconds in (a negative or non-finite time reads as 0), the whole seconds left in it, and how far the
/// motion got (frozen at 60 once it ended).
pub fn at(elapsed: f64) -> Moment {
    let t = if elapsed.is_finite() { elapsed.max(0.0) } else { 0.0 };
    let motion = (t - PREROLL_SECONDS).clamp(0.0, MOTION_SECONDS);
    let (phase, end) = if t < 3.0 {
        ("preroll", 3.0)
    } else if t < 63.0 {
        ("motion", 63.0)
    } else if t < 93.0 {
        ("still", 93.0)
    } else {
        ("done", t)
    };
    Moment { phase, seconds_left: (end - t).ceil() as i32, motion_elapsed: motion }
}

/// `ScrollOffset`: the document's scroll after `motion_elapsed` seconds, wrapping at its height.
pub fn scroll_offset(motion_elapsed: f64, content_height: f64) -> f64 {
    if content_height.is_finite() && content_height > 0.0 && motion_elapsed.is_finite() {
        motion_elapsed.max(0.0) * SCROLL_POINTS_PER_SECOND % content_height
    } else {
        0.0
    }
}

/// `TypedCount`: the terminal's characters typed after `motion_elapsed` seconds.
pub fn typed_count(motion_elapsed: f64) -> i32 {
    if motion_elapsed.is_finite() {
        (motion_elapsed.max(0.0) * TYPED_CHARACTERS_PER_SECOND).floor().min(f64::from(i32::MAX)) as i32
    } else {
        0
    }
}

/// `Announcement`: the data channel's `{t:"scene", phase}` for one of the four phases; none for any other word (C#'s
/// `ArgumentException`).
pub fn announcement(phase: &str) -> Option<[(&'static str, &'static str); 2]> {
    let phase = match phase {
        "preroll" => "preroll",
        "motion" => "motion",
        "still" => "still",
        "done" => "done",
        _ => return None,
    };
    Some([("t", "scene"), ("phase", phase)])
}
