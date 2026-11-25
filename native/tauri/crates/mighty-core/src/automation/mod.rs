//! MC-32: schedules and event triggers that start agent work on their own. Pure parts only; the app (src-tauri `automation`) runs
//! the timer, the watches and the fires.
//!
//! - [`recurrence`]: a 5-field cron line or an RRULE subset, and the next instant it names in a time zone (DST, month ends).
//! - [`spec`]: what a schedule and a trigger are (the same shapes from `.mighty.yaml` and from settings) and their checks.
//! - [`condition`]: MC-97's `where`: a trigger's conditions over its event's fields and envelope, and why an event missed them.
//! - [`mapping`]: MC-98's reading of a JSON body into an event: a webhook's `map` and its presets (`generic`, `github`,
//!   `mightywork`).
//! - [`mcp_watch`]: MC-98's MCP polling source: a tool's list of items compared with the last look's (created, a watched field
//!   changed, a label added) and the events that makes.
//! - [`policy`]: MC-99's trigger policy (approval, once per subject, concurrency, cooldown, a daily cap) and what it decides.
//! - [`action`]: MC-99's requests a `develop` and a `run` send, and a `run`'s command and an `emit`'s data filled from the event.
//! - [`engine`]: what a fire does to the history (its record, pane, card, end, cost, result link) and which job a retry fires.
//! - [`plan`]: which schedules are due with the clock given, the catch-up rule for fires the app missed, and when to wake next.
//! - [`records`]: the run history, capped by count and size, and a pane's result link.
//! - [`wake`]: MC-39's OS wake-up for the next fire while the app is closed (the LaunchAgent, the scheduled task), when to register
//!   it again, and when a server the OS started for schedules quits.
//! - [`watch`]: what the triggers keep between looks — seen delivery ids, the poll's back-off and ETags, a standing order's output
//!   and daily cap, Linear issues' states — and the prompt a trigger's event makes.

pub mod action;
pub mod condition;
pub mod engine;
pub mod mapping;
pub mod mcp_watch;
pub mod plan;
pub mod policy;
pub mod records;
pub mod recurrence;
pub mod spec;
pub mod wake;
pub mod watch;

use std::path::Path;

/// Writes `bytes` to `path` whole: a temporary file beside it, then renamed over it (the folder made first).
pub fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let temporary = path.with_extension(format!("tmp-{}", std::process::id()));
    std::fs::write(&temporary, bytes)?;
    std::fs::rename(&temporary, path).inspect_err(|_| {
        let _ = std::fs::remove_file(&temporary);
    })
}
