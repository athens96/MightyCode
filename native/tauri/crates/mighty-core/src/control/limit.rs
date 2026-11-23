//! The webhooks' debounce and rate limit (MC-9), with the time given so tests need no clock.

use std::collections::HashMap;
use std::time::{Duration, Instant};

/// A token bucket per key: `capacity` sends at once, refilled evenly over `per`.
#[derive(Debug)]
pub struct RateLimiter {
    capacity: f64,
    per: Duration,
    buckets: HashMap<String, (f64, Instant)>,
}

impl RateLimiter {
    pub fn new(capacity: u32, per: Duration) -> RateLimiter {
        RateLimiter { capacity: f64::from(capacity.max(1)), per, buckets: HashMap::new() }
    }

    /// Whether `key` may send at `now`; a send it allows takes one token.
    pub fn allow(&mut self, key: &str, now: Instant) -> bool {
        let rate = self.capacity / self.per.as_secs_f64().max(f64::EPSILON);
        let capacity = self.capacity;
        let (tokens, last) = self.buckets.entry(key.to_string()).or_insert((capacity, now));
        let elapsed = now.saturating_duration_since(*last).as_secs_f64();
        *tokens = (*tokens + elapsed * rate).min(capacity);
        *last = now;
        if *tokens >= 1.0 {
            *tokens -= 1.0;
            true
        } else {
            false
        }
    }
}

/// The same key again within `window` is dropped.
#[derive(Debug)]
pub struct Debouncer {
    window: Duration,
    last: HashMap<String, Instant>,
}

impl Debouncer {
    pub fn new(window: Duration) -> Debouncer {
        Debouncer { window, last: HashMap::new() }
    }

    /// Whether `key` passes at `now` (and, when it does, the window starts again from `now`).
    pub fn allow(&mut self, key: &str, now: Instant) -> bool {
        let window = self.window;
        // Keys older than the window are forgotten, so the map stays small.
        self.last.retain(|_, at| now.saturating_duration_since(*at) < window);
        if self.last.contains_key(key) {
            return false;
        }
        self.last.insert(key.to_string(), now);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_rate_limit_allows_a_burst_then_refills_over_time() {
        let start = Instant::now();
        let mut limiter = RateLimiter::new(3, Duration::from_secs(60));
        assert!((0..3).all(|_| limiter.allow("a", start)));
        assert!(!limiter.allow("a", start), "the fourth at once is refused");
        assert!(limiter.allow("b", start), "each key has its own bucket");
        assert!(!limiter.allow("a", start + Duration::from_secs(10)), "a third of a token is not enough");
        assert!(limiter.allow("a", start + Duration::from_secs(21)), "one token back after 20 s");
        assert!(!limiter.allow("a", start + Duration::from_secs(22)));
        assert!((0..3).all(|_| limiter.allow("a", start + Duration::from_secs(600))), "never more than the capacity");
        assert!(!limiter.allow("a", start + Duration::from_secs(600)));
    }

    #[test]
    fn the_debounce_drops_repeats_within_its_window() {
        let start = Instant::now();
        let mut debounce = Debouncer::new(Duration::from_secs(5));
        assert!(debounce.allow("p1:done", start));
        assert!(!debounce.allow("p1:done", start + Duration::from_secs(4)));
        assert!(debounce.allow("p1:failed", start + Duration::from_secs(4)));
        assert!(debounce.allow("p1:done", start + Duration::from_secs(5)));
        assert!(debounce.allow("p2:done", start + Duration::from_secs(30)));
        assert_eq!(debounce.last.len(), 1, "older keys are forgotten");
    }
}
