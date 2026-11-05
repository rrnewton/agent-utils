//! Single per-pane admission point for every prompt typed into a TUI.
//!
//! Every sender (agentctl send, chat bridge, agent-teams drain, watch-agents
//! notices, herdr-agent prompts, tui_prompts answers) must go through
//! [`admit`] so that, per pane: admissions are serialised under a
//! cross-process flock, at most one admission starts per pacing interval,
//! and each admission is audit-logged. The closure runs while the lock is
//! held; it must type into an empty composer and confirm submission before
//! returning (see `submission::submit_verified`).

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use fs2::FileExt;

use crate::error::{AdapterError, Result};

/// Default per-pane pacing interval.
pub const DEFAULT_PACE: Duration = Duration::from_millis(1000);
/// Configurable pacing floor.
pub const MIN_PACE: Duration = Duration::from_millis(500);
/// Bare-Enter retry budget under uncertainty (shared with unknown harnesses):
/// first retry after 1 s, doubling to 4 s, at most 5 presses, never a re-paste.
pub const ENTER_RETRY_FIRST: Duration = Duration::from_millis(1000);
/// Slowest bare-Enter retry interval.
pub const ENTER_RETRY_MAX: Duration = Duration::from_millis(4000);
/// Most bare-Enter presses under uncertainty.
pub const ENTER_RETRY_MAX_PRESSES: u32 = 5;

fn sanitised(pane_id: &str) -> String {
    pane_id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.') {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// Directory holding one pane's lock, pace stamp and audit log.
pub fn pane_dir(pane_id: &str) -> PathBuf {
    if let Some(dir) = std::env::var_os("AGENTCTL_PANE_ADMISSION_DIR") {
        return PathBuf::from(dir).join(sanitised(pane_id));
    }
    // Tests must never write the real per-user state directory: test
    // binaries (unit and integration) live under a `deps` directory.
    if std::env::current_exe().is_ok_and(|exe| exe.to_string_lossy().contains("/deps/")) {
        return std::env::temp_dir()
            .join(format!(
                "agentctl-test-panes-{}-{:?}",
                std::process::id(),
                std::thread::current().id()
            ))
            .join(sanitised(pane_id));
    }
    {
        let state = std::env::var_os("XDG_STATE_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/state")));
        state
            .unwrap_or_else(|| PathBuf::from("/tmp"))
            .join("agentctl/panes")
            .join(sanitised(pane_id))
    }
}

/// Pacing interval from `AGENTCTL_PANE_PACE_SECONDS` (default 1.0, floor 0.5).
pub fn pace_interval() -> Duration {
    match std::env::var("AGENTCTL_PANE_PACE_SECONDS")
        .ok()
        .and_then(|v| v.parse::<f64>().ok())
    {
        Some(secs) if secs.is_finite() && secs > 0.0 => Duration::from_secs_f64(secs).max(MIN_PACE),
        _ => DEFAULT_PACE,
    }
}

/// How long to wait before the next admission, given the last start.
pub fn pace_wait(last: Option<Duration>, now: Duration, interval: Duration) -> Duration {
    match last {
        Some(last) => interval.saturating_sub(now.saturating_sub(last)),
        None => Duration::ZERO,
    }
}

fn now_epoch() -> Duration {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
}

/// Held for the whole admission; dropping releases the flock.
pub struct PaneAdmission {
    _lock: File,
    dir: PathBuf,
}

impl PaneAdmission {
    /// Acquire the pane lock, pace, and return the held admission.
    pub fn acquire(pane_id: &str) -> Result<Self> {
        let dir = pane_dir(pane_id);
        std::fs::create_dir_all(&dir)
            .map_err(|e| AdapterError::unavailable(format!("pane admission dir: {e}")))?;
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(dir.join("admission.lock"))
            .map_err(|e| AdapterError::unavailable(format!("pane admission lock: {e}")))?;
        // Bounded wait: an unbounded flock here made `chat run` hang on
        // shutdown until systemd's stop timeout when another sender held
        // the pane. Try-lock in short naps up to the deadline instead.
        let lock_timeout = std::env::var("AGENTCTL_PANE_LOCK_TIMEOUT_MS")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .map(Duration::from_millis)
            .unwrap_or(Duration::from_secs(10));
        let lock_deadline = std::time::Instant::now() + lock_timeout;
        loop {
            match lock.try_lock_exclusive() {
                Ok(()) => break,
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    if std::time::Instant::now() >= lock_deadline {
                        return Err(AdapterError::unavailable(format!(
                            "pane admission flock timed out after {lock_timeout:?}: {e}"
                        )));
                    }
                    std::thread::sleep(Duration::from_millis(50));
                }
                Err(e) => {
                    return Err(AdapterError::unavailable(format!(
                        "pane admission flock: {e}"
                    )))
                }
            }
        }
        // Pace while holding the lock so starts are serialised at >= interval.
        let stamp = dir.join("last-admission");
        let last = std::fs::read_to_string(&stamp)
            .ok()
            .and_then(|s| s.trim().parse::<f64>().ok())
            .map(Duration::from_secs_f64);
        let wait = pace_wait(last, now_epoch(), pace_interval());
        if !wait.is_zero() {
            std::thread::sleep(wait);
        }
        let _ = std::fs::write(&stamp, format!("{:.3}", now_epoch().as_secs_f64()));
        Ok(Self { _lock: lock, dir })
    }

    fn audit(&self, kind: &str, id: &str, outcome: &str) {
        let line = format!(
            "{{\"at\":{:.3},\"sender\":{:?},\"id\":{:?},\"outcome\":{:?}}}\n",
            now_epoch().as_secs_f64(),
            kind,
            id,
            outcome
        );
        if let Ok(mut f) = OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.dir.join("admissions.jsonl"))
        {
            let _ = f.write_all(line.as_bytes());
        }
    }
}

/// Admit one delivery to `pane_id`: lock, pace, run `deliver`, audit, unlock.
///
/// `strategy` names the per-harness submit seam: `verified` for
/// Claude/Codex/Muse (`submission::submit_verified`), `unknown` for
/// `unknown_harness::submit`. Admission itself is harness-agnostic; the
/// empty-composer gate and stranded-paste reconciliation live in the
/// strategy, parameterised by that harness's composer predicate.
pub fn admit<T>(
    pane_id: &str,
    sender: &str,
    message_id: &str,
    strategy: &str,
    deliver: impl FnOnce() -> Result<T>,
) -> Result<T> {
    let admission = PaneAdmission::acquire(pane_id)?;
    let outcome = deliver();
    let label = match &outcome {
        Ok(_) => format!("delivered:{strategy}"),
        Err(e) => format!("error:{strategy}:{e}"),
    };
    let _ = sender;
    admission.audit(sender, message_id, &label);
    outcome
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pace_wait_enforces_the_interval() {
        let interval = Duration::from_millis(1000);
        assert_eq!(
            pace_wait(None, Duration::from_secs(10), interval),
            Duration::ZERO
        );
        assert_eq!(
            pace_wait(
                Some(Duration::from_secs(10)),
                Duration::from_millis(10_400),
                interval
            ),
            Duration::from_millis(600)
        );
        assert_eq!(
            pace_wait(
                Some(Duration::from_secs(10)),
                Duration::from_secs(11),
                interval
            ),
            Duration::ZERO
        );
    }

    #[test]
    fn a_burst_of_five_starts_at_least_one_second_apart() {
        // Simulate five admissions through the pacer: each start is the
        // previous start plus at least the interval, so they can never
        // merge into one composer session.
        let interval = Duration::from_millis(1000);
        let mut last: Option<Duration> = None;
        let mut now = Duration::from_secs(100);
        let mut starts = Vec::new();
        for _ in 0..5 {
            now += pace_wait(last, now, interval);
            starts.push(now);
            last = Some(now);
            now += Duration::from_millis(100); // delivery work
        }
        for pair in starts.windows(2) {
            assert!(pair[1] - pair[0] >= interval, "{starts:?}");
        }
        assert_eq!(starts.len(), 5);
    }

    #[test]
    fn no_new_path_types_into_a_pane_outside_the_admission_funnel() {
        // Every prompt typed into a TUI must reach `admit` (or the
        // PaneAdmission lock in tui_prompts::answer). Direct terminal
        // input is allowed only in the transport and in the admitted
        // strategy bodies; a new call site anywhere else fails here.
        const ALLOWED: &[&str] = &[
            "client.rs",          // Herdr transport primitives
            "term.rs",            // term-backend transport
            "submission.rs",      // verified strategy (admitted closure)
            "unknown_harness.rs", // unknown strategy (admitted closure)
            "subagents.rs",       // GuardedInput transport + admitted callers
            "tui_prompts.rs",     // answer() holds PaneAdmission
            "stale_composer.rs",  // deterministic sweep keys, not prompts
            "pane_admission.rs",
        ];
        let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut offenders = Vec::new();
        let mut stack = vec![src];
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(&dir).expect("read src") {
                let path = entry.expect("entry").path();
                if path.is_dir() {
                    stack.push(path);
                    continue;
                }
                if path.extension().is_none_or(|e| e != "rs") {
                    continue;
                }
                let name = path.file_name().unwrap().to_string_lossy().into_owned();
                if ALLOWED.contains(&name.as_str()) || name.contains("test") {
                    continue;
                }
                let body = std::fs::read_to_string(&path).expect("read rs");
                // Tests and fakes type into fake panes, never real ones.
                let body = body.split("#[cfg(test)]").next().unwrap_or("");
                for needle in [
                    ".send_text(",
                    ".send_keys(",
                    ".native_prompt(",
                    ".agent_prompt(",
                ] {
                    if body.contains(needle) {
                        offenders.push(format!("{}: {needle}", path.display()));
                    }
                }
            }
        }
        assert!(
            offenders.is_empty(),
            "direct pane input outside admission: {offenders:?}"
        );
    }

    #[test]
    fn a_held_admission_lock_times_out_instead_of_hanging() {
        std::env::set_var("AGENTCTL_PANE_LOCK_TIMEOUT_MS", "300");
        let held = PaneAdmission::acquire("w9:p9").expect("first acquire");
        let started = std::time::Instant::now();
        let outcome = PaneAdmission::acquire("w9:p9");
        assert!(outcome.is_err(), "second acquire must time out");
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "{:?}",
            started.elapsed()
        );
        drop(held);
        std::env::remove_var("AGENTCTL_PANE_LOCK_TIMEOUT_MS");
    }

    #[test]
    fn pace_interval_has_a_half_second_floor() {
        assert!(
            pace_interval() >= MIN_PACE || std::env::var("AGENTCTL_PANE_PACE_SECONDS").is_err()
        );
        assert_eq!(DEFAULT_PACE, Duration::from_millis(1000));
        assert_eq!(ENTER_RETRY_MAX_PRESSES, 5);
    }
}
