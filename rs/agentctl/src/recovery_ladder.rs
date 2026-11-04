//! Deterministic recovery ladder state (phase 2): no LLM calls, bounded.
//!
//! Steps, per agent episode: (1) stale-composer sweep, (2) redraw,
//! (3) unreadable/blank classification, (4) after 3 failed sweep/redraw
//! cycles and 10 min unrecoverable, one child-first `--resume` restart
//! (at most 1 per agent per hour), re-anchoring every registry pin.
//! Every step is logged and notified once by the caller (watch-agents).

use std::time::Duration;

/// Minutes unrecoverable before the restart step.
pub const LADDER_MINUTES: u64 = 10;
/// Failed sweep/redraw cycles required before a restart.
pub const RESTART_AFTER_CYCLES: u32 = 3;
/// Most restarts per agent per hour.
pub const MAX_RESTARTS_PER_HOUR: u32 = 1;

/// One ladder decision for an episode.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LadderStep {
    /// Submit/clear a stranded own draft (bare Enter flush first).
    Sweep,
    /// Ask the terminal for a redraw and re-read.
    Redraw,
    /// TERM the harness child and relaunch with `--resume`, re-anchor pins.
    Restart,
    /// Budget exhausted: stop and ask an operator.
    OperatorRequired,
}

/// Pure ladder state for one agent.
#[derive(Clone, Debug, Default)]
pub struct Ladder {
    /// Failed sweep/redraw cycles in this episode.
    pub failed_cycles: u32,
    /// Restarts in the last hour.
    pub restarts_last_hour: u32,
    /// Episode age.
    pub unrecoverable_for: Duration,
}

impl Ladder {
    /// Next step for the current state.
    pub fn next_step(&self) -> LadderStep {
        if self.failed_cycles >= RESTART_AFTER_CYCLES
            && self.unrecoverable_for >= Duration::from_secs(LADDER_MINUTES * 60)
        {
            if self.restarts_last_hour >= MAX_RESTARTS_PER_HOUR {
                return LadderStep::OperatorRequired;
            }
            return LadderStep::Restart;
        }
        if self.failed_cycles % 2 == 0 {
            LadderStep::Sweep
        } else {
            LadderStep::Redraw
        }
    }

    /// Record a failed sweep/redraw cycle.
    pub fn record_failure(&mut self) {
        self.failed_cycles = self.failed_cycles.saturating_add(1);
    }

    /// Record a restart (resets the cycle count, keeps the hourly budget).
    pub fn record_restart(&mut self) {
        self.restarts_last_hour = self.restarts_last_hour.saturating_add(1);
        self.failed_cycles = 0;
    }
}

/// Whether a delivery error marks a stranded prompt for the ladder:
/// our still-staged errors and the generic `stranded_prompt:` marker.
pub fn is_stranded_prompt(detail: &str) -> bool {
    // One stranded representation: the queue's stranded marker (#850).
    crate::unknown_harness::is_stranded_prompt(detail)
        || detail.contains("still staged in the")
        || detail.contains("composer already holds unsubmitted text")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ladder_escalates_only_after_three_cycles_and_ten_minutes() {
        let mut ladder = Ladder::default();
        assert_eq!(ladder.next_step(), LadderStep::Sweep);
        ladder.record_failure();
        assert_eq!(ladder.next_step(), LadderStep::Redraw);
        ladder.record_failure();
        ladder.record_failure();
        ladder.unrecoverable_for = Duration::from_secs(599);
        assert_ne!(ladder.next_step(), LadderStep::Restart);
        ladder.unrecoverable_for = Duration::from_secs(600);
        assert_eq!(ladder.next_step(), LadderStep::Restart);
        ladder.record_restart();
        assert_eq!(ladder.next_step(), LadderStep::Sweep);
        ladder.failed_cycles = 3;
        ladder.unrecoverable_for = Duration::from_secs(600);
        assert_eq!(ladder.next_step(), LadderStep::OperatorRequired);
    }

    #[test]
    fn stranded_markers_are_classified() {
        assert!(is_stranded_prompt("x stranded_prompt: y"));
        assert!(is_stranded_prompt(
            "prompt is still staged in the claude composer"
        ));
        assert!(!is_stranded_prompt("pane did not become idle"));
    }
}
