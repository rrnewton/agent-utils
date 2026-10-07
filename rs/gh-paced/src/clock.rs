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
    /// Seconds from an arbitrary origin on a clock that never moves backwards, for measuring
    /// how long something has taken: setting the system time back or forward does not change
    /// it, and time the machine spends suspended counts. The default is [`Clock::now`], which
    /// suits clocks that only move forward.
    fn monotonic(&self) -> f64 {
        self.now()
    }
    /// Block for `secs` seconds of [`Clock::monotonic`] (non-positive values return at once),
    /// so that time the machine spends suspended during the sleep counts against it, which
    /// [`Clock::sleep`] need not do. The default is [`Clock::sleep`].
    fn sleep_monotonic(&self, secs: f64) {
        self.sleep(secs)
    }
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

    /// [`boottime`].
    fn monotonic(&self) -> f64 {
        boottime()
    }

    /// Sleeps until [`boottime`] has advanced by `secs` ([`sleep_boottime_until`]).
    /// `std::thread::sleep`, used by [`Clock::sleep`], times on `CLOCK_MONOTONIC`, which stops
    /// while the machine is suspended, so a suspend would lengthen it by the time suspended.
    fn sleep_monotonic(&self, secs: f64) {
        if secs > 0.0 && secs.is_finite() {
            sleep_boottime_until(boottime() + secs);
        }
    }
}

/// Seconds since boot on Linux's `CLOCK_BOOTTIME`, which never moves backwards, is not changed
/// by setting the system time, and keeps counting while the machine is suspended.
/// `std::time::Instant` reads `CLOCK_MONOTONIC` instead, which stops during suspend, so a wait
/// measured with it would leave out the time the machine slept.
///
/// If the kernel refuses `CLOCK_BOOTTIME` (Linux has had it since 2.6.39), this reads
/// `CLOCK_MONOTONIC`, which starts from the same boot origin but leaves out suspended time.
/// Linux always has `CLOCK_MONOTONIC`; `std::time::Instant::now` panics without it, and so does
/// this.
pub fn boottime() -> f64 {
    read_clock(libc::CLOCK_BOOTTIME)
        .or_else(|| read_clock(libc::CLOCK_MONOTONIC))
        .expect("clock_gettime(CLOCK_MONOTONIC) failed")
}

/// Block until [`boottime`] reads at least `deadline`, with an absolute `clock_nanosleep` timer
/// on `CLOCK_BOOTTIME`, which keeps running while the machine is suspended, restarted after a
/// signal. If the kernel refuses that timer, sleeps with `std::thread::sleep` for what
/// [`boottime`] says is left until it has reached the deadline.
pub fn sleep_boottime_until(deadline: f64) {
    if deadline.is_finite() && deadline > 0.0 {
        let ts = libc::timespec {
            tv_sec: deadline.trunc() as libc::time_t,
            tv_nsec: ((deadline.fract() * 1e9) as libc::c_long).clamp(0, 999_999_999),
        };
        loop {
            // SAFETY: clock_nanosleep reads one valid timespec; with TIMER_ABSTIME it writes no
            // remainder, so that pointer may be null.
            let rc = unsafe {
                libc::clock_nanosleep(
                    libc::CLOCK_BOOTTIME,
                    libc::TIMER_ABSTIME,
                    &ts,
                    std::ptr::null_mut(),
                )
            };
            match rc {
                0 => return,
                libc::EINTR => continue,
                _ => break,
            }
        }
    }
    loop {
        let left = deadline - boottime();
        if !(left > 0.0 && left.is_finite()) {
            return;
        }
        std::thread::sleep(Duration::from_secs_f64(left));
    }
}

fn read_clock(id: libc::clockid_t) -> Option<f64> {
    let mut ts = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: clock_gettime writes one timespec through a valid, exclusive pointer.
    let rc = unsafe { libc::clock_gettime(id, &mut ts) };
    (rc == 0).then(|| ts.tv_sec as f64 + ts.tv_nsec as f64 / 1e9)
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
        assert_eq!(
            clock.monotonic(),
            200.0,
            "a fake clock measures with its own time"
        );
    }

    #[test]
    fn real_clock_monotonic_advances_with_sleeps() {
        let clock = RealClock;
        let before = clock.monotonic();
        clock.sleep(0.05);
        let took = clock.monotonic() - before;
        assert!((0.05..5.0).contains(&took), "measured {took} s");
    }

    /// The real clock's monotonic sleep lasts the time asked for on boot time, and returns at
    /// once for a non-positive or non-finite time, as [`Clock::sleep`] does. (This host cannot be
    /// suspended from a test, so that a suspend counts is not exercised here.)
    #[test]
    fn real_clock_sleep_monotonic_sleeps_on_boot_time() {
        let clock = RealClock;
        let before = boottime();
        clock.sleep_monotonic(0.05);
        let took = boottime() - before;
        assert!((0.05..5.0).contains(&took), "slept {took} s");
        let before = boottime();
        for secs in [0.0, -1.0, f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            clock.sleep_monotonic(secs);
        }
        sleep_boottime_until(before - 10.0);
        sleep_boottime_until(f64::NAN);
        let took = boottime() - before;
        assert!(took < 1.0, "returned at once: {took} s");
    }

    /// The real clock measures on boot time, the clock that keeps counting while the machine is
    /// suspended. `/proc/uptime` is the kernel's own report of that clock (fs/proc/uptime.c reads
    /// it with `ktime_get_boottime_ts64` and applies the same time-namespace offset as
    /// `clock_gettime`), truncated to hundredths of a second, so the real clock must read it
    /// exactly when compared within that resolution. A clock measured from the process start
    /// (`Instant` elapsed) reads near zero here and fails; plain `CLOCK_MONOTONIC` fails too
    /// once the machine has been suspended, because it leaves the suspended time out.
    #[test]
    fn real_clock_monotonic_is_boot_time() {
        let clock = RealClock;
        let before = clock.monotonic();
        let text = std::fs::read_to_string("/proc/uptime").expect("read /proc/uptime");
        let after = clock.monotonic();
        let uptime: f64 = text
            .split_whitespace()
            .next()
            .and_then(|f| f.parse().ok())
            .expect("parse /proc/uptime");
        assert!(
            before - 0.011 <= uptime && uptime <= after,
            "real clock read {before}..{after} s around /proc/uptime {uptime} s"
        );
    }
}
