//! Pre-flight checks over a DAG configuration.

use crate::model::{resolved_wall_timeout, DagConfig};

/// Steps whose own wall budget is not STRICTLY SMALLER than the run's outer budget.
///
/// INNER < OUTER IS THE WHOLE ORDERING, and it is checkable before anything runs. A step allowed
/// to run as long as (or longer than) the run itself can only ever be terminated by the outer
/// bound, which means the failure is attributed to "the run overran" instead of to the node that
/// caused it — precisely the report that made a real regression unexplainable. Returns the
/// offending `(tag, step_timeout_s)` pairs, empty when the ordering holds.
pub fn steps_violating_run_timeout(cfg: &DagConfig, run_timeout_s: i64) -> Vec<(String, i64)> {
    if run_timeout_s <= 0 {
        return Vec::new();
    }
    // The RESOLVED bound, not the declared field. A step that declares no wall budget carries
    // the 0 sentinel, which would pass a `>= run_timeout_s` test trivially while the value it
    // will actually run under — derived from its CPU budget, or 1800 — might not.
    let mut bad: Vec<(String, i64)> = cfg
        .steps
        .iter()
        .map(|s| {
            (
                s.tag(),
                resolved_wall_timeout(s, cfg.default_step_timeout, cfg.cpu_timeout_multiplier),
            )
        })
        .filter(|(_, bound)| *bound >= run_timeout_s)
        .collect();
    bad.sort();
    bad
}
