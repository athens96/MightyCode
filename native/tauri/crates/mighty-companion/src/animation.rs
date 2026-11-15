//! The animation rows and `Priority` (C# Core\Companion.cs:137-153 `CompanionAnimation`; plan §1.7): row 6 while the user is waited on,
//! 5 after a failure, 4 celebrating a completed run (for the 6 s its bubble shows), 8 reading or reviewing, 7 at other work, else 0.
//! The 1/12 s redraw and the 12th-tick refresh are the app's (src-tauri companion.rs); [`crate::pet::frame`] times the frames. Owner:
//! W8-E.

use mighty_core::model::AgentActivity;

/// `CompanionAnimation.Priority(status)`: which agent the pet follows when nobody picked one (the highest, then the latest seen).
pub fn priority(status: &str) -> i32 {
    match status {
        "waiting" => 5,
        "running" | "starting" | "queued" => 4,
        "error" => 3,
        "completed" => 2,
        _ => 1,
    }
}

/// `CompanionAnimation.Row(status, activity, celebrating)`.
pub fn row(status: &str, activity: Option<&AgentActivity>, celebrating: bool) -> i32 {
    match status {
        "waiting" => return 6,
        "error" | "failed" | "stopped" | "cancelled" => return 5,
        "completed" => return if celebrating { 4 } else { 0 },
        "running" | "starting" | "queued" => {}
        _ => return 0,
    }
    if let Some(activity) = activity {
        if activity.state == "waiting" {
            return 6;
        }
        if activity.state == "error" {
            return 5;
        }
        if matches!(activity.kind.as_str(), "read" | "search" | "web" | "review") {
            return 8;
        }
        if matches!(activity.kind.as_str(), "edit" | "write" | "command" | "thinking" | "agent" | "turn") {
            return 7;
        }
    }
    let summary = activity.map(|a| a.summary.to_lowercase()).unwrap_or_default();
    let reading = ["read", "search", "review", "inspect", "\u{C77D}", "\u{AC80}\u{C0C9}", "\u{AC80}\u{D1A0}", "\u{CC3E}\u{B294}"];
    if reading.iter().any(|word| summary.contains(word)) { 8 } else { 7 }
}
