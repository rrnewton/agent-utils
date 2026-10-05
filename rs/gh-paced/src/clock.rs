//! Wall-clock access behind a trait, so the pacing engine can be driven by a fake clock in tests.

use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Source of the current time and of blocking sleeps.
///
/// Times are Unix seconds as `f64`. The production binary always uses [`RealClock`]; there is no
/// environment variable or flag that substitutes another clock, so the budgets cannot be bypassed
/// by lying about the time.
pub trait Clock: Send + Sync {
    /// Current time in Unix seconds.
    fn now(&self) -> f64;
    /// Block for `secs` seconds (non-positive values return at once).
    fn sleep(&self, secs: f64);
}

/// The system clock.
#[derive(Debug, Default, Clone, Copy)]
pub struct RealClock;

impl Clock for RealClock {
    fn now(&self) -> f64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs_f64())
            .unwrap_or(0.0)
    }

    fn sleep(&self, secs: f64) {
        if secs > 0.0 && secs.is_finite() {
            std::thread::sleep(Duration::from_secs_f64(secs));
        }
    }
}

/// A deterministic clock for tests: `sleep` advances the time instantly and is recorded.
#[derive(Debug)]
pub struct FakeClock {
    now: Mutex<f64>,
    sleeps: Mutex<Vec<f64>>,
}

impl FakeClock {
    /// A fake clock starting at `start` Unix seconds.
    pub fn new(start: f64) -> Self {
        Self {
            now: Mutex::new(start),
            sleeps: Mutex::new(Vec::new()),
        }
    }

    /// Move the clock to `t` if `t` is later than the current time (never backwards).
    pub fn advance_to(&self, t: f64) {
        let mut now = self.now.lock().unwrap_or_else(|e| e.into_inner());
        if t > *now {
            *now = t;
        }
    }

    /// Every sleep requested so far, in seconds, in order.
    pub fn sleeps(&self) -> Vec<f64> {
        self.sleeps
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// Total seconds slept so far.
    pub fn total_slept(&self) -> f64 {
        self.sleeps().iter().sum()
    }
}

impl Clock for FakeClock {
    fn now(&self) -> f64 {
        *self.now.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn sleep(&self, secs: f64) {
        if secs > 0.0 && secs.is_finite() {
            self.sleeps
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(secs);
            let mut now = self.now.lock().unwrap_or_else(|e| e.into_inner());
            *now += secs;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fake_clock_sleep_advances_and_records() {
        let clock = FakeClock::new(100.0);
        clock.sleep(2.5);
        clock.sleep(0.0);
        clock.sleep(-1.0);
        assert_eq!(clock.now(), 102.5);
        assert_eq!(clock.sleeps(), vec![2.5]);
        clock.advance_to(50.0);
        assert_eq!(clock.now(), 102.5, "advance_to never moves backwards");
        clock.advance_to(200.0);
        assert_eq!(clock.now(), 200.0);
    }
}
