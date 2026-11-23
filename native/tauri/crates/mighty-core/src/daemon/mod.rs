//! MC-3: the background daemon's pure parts. The app (src-tauri `daemon`) and the engine (mighty-run `daemon`) host them.
//!
//! - [`protocol`]: the app↔daemon wire, in the control API's style (MC-9): one JSON object per line over the profile's own socket or
//!   pipe, every line capped. The first request is `hello` with the token and the protocol version; the daemon then answers each
//!   request with one `ok` or `error` line and writes `event` lines (output, exit) for the processes a connection attached to.
//! - [`ring`]: a process's output kept in a size-capped ring with a byte sequence, so a client that comes back asks for what came
//!   after the last byte it saw ([`ring::Cursor`] takes each byte exactly once).
//! - [`IdleClock`]: when a daemon with nothing running and nobody attached exits.

pub mod protocol;
pub mod ring;

use std::time::{Duration, Instant};

/// How long a daemon with no running process and no client waits before it exits on its own.
pub const IDLE_EXIT: Duration = Duration::from_secs(10 * 60);
/// How long it waits instead while it keeps the end of a kept run nobody has read yet (the app may come back for it).
pub const HELD_EXIT: Duration = Duration::from_secs(24 * 60 * 60);

/// When a daemon is idle: no running process and no client, since a moment it remembers. While it holds output of an ended kept run
/// that no app has read, it waits longer ([`HELD_EXIT`] at least). The time is given, so tests need no clock.
#[derive(Debug, Clone)]
pub struct IdleClock {
    after: Duration,
    idle_since: Option<Instant>,
    held: bool,
}

impl IdleClock {
    pub fn new(after: Duration, now: Instant) -> IdleClock {
        IdleClock { after, idle_since: Some(now), held: false }
    }

    /// What the daemon holds now: the clock starts when both reach zero and stops when either is not.
    pub fn update(&mut self, running: usize, clients: usize, now: Instant) {
        if running == 0 && clients == 0 {
            self.idle_since.get_or_insert(now);
        } else {
            self.idle_since = None;
        }
    }

    /// Whether it keeps the unread end of a kept run.
    pub fn hold(&mut self, held: bool) {
        self.held = held;
    }

    /// When the daemon exits unless something changes first; None while it is not idle.
    pub fn deadline(&self) -> Option<Instant> {
        let after = if self.held { self.after.max(HELD_EXIT) } else { self.after };
        self.idle_since.map(|since| since + after)
    }

    /// Whether the daemon should exit at `now`.
    pub fn expired(&self, now: Instant) -> bool {
        self.deadline().is_some_and(|deadline| now >= deadline)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_idle_clock_runs_only_with_nothing_running_and_nobody_attached() {
        let start = Instant::now();
        let after = Duration::from_secs(600);
        let mut clock = IdleClock::new(after, start);
        assert_eq!(clock.deadline(), Some(start + after), "a daemon nobody uses goes on its own");
        clock.update(1, 0, start + Duration::from_secs(5));
        assert_eq!(clock.deadline(), None, "a running process keeps it");
        clock.update(0, 1, start + Duration::from_secs(10));
        assert_eq!(clock.deadline(), None, "an attached app keeps it");
        clock.update(0, 0, start + Duration::from_secs(20));
        assert!(!clock.expired(start + Duration::from_secs(619)));
        assert!(clock.expired(start + Duration::from_secs(620)), "the clock started when the last client went");
        clock.update(0, 0, start + Duration::from_secs(700));
        assert_eq!(clock.deadline(), Some(start + Duration::from_secs(620)), "staying idle does not restart it");
        clock.hold(true);
        assert_eq!(clock.deadline(), Some(start + Duration::from_secs(20) + HELD_EXIT), "an unread kept run's end waits for its app");
        clock.hold(false);
        assert!(clock.expired(start + Duration::from_secs(620)));
    }
}
