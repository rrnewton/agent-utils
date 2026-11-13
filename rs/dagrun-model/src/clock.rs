//! Monotonic clock reads that survive an exec.

/// Read Linux's process-independent monotonic clock in nanoseconds.
///
/// `Instant` cannot be serialized through an exec.  `CLOCK_MONOTONIC` is the same boot-scoped
/// clock class and remains stable across fork/exec without inheriting wall-clock adjustments.
pub fn monotonic_now_ns() -> Option<u64> {
    let mut ts = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    if unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut ts) } != 0
        || ts.tv_sec < 0
        || ts.tv_nsec < 0
    {
        return None;
    }
    (ts.tv_sec as u64)
        .checked_mul(1_000_000_000)
        .and_then(|s| s.checked_add(ts.tv_nsec as u64))
}
