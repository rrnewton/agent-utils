//! The `local-ci-hub/run/v1` run handle: one record per run.
//!
//! The Python edition writes handles; this type is the single
//! schema source both editions validate against. Verdict state and
//! cleanup state are orthogonal: a run reaches its terminal verdict
//! when its record is written, and cleanup afterwards only moves
//! the cleanup track, never the verdict fields.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Handle schema tag every writer stamps.
pub const SCHEMA: &str = "local-ci-hub/run/v1";

/// Verdict lifecycle of one run, in wire spelling.
///
/// `Finished`, `Stopped` and `Stale` are the pre-lifecycle spellings
/// the Python prototype already writes; new writers use the
/// lifecycle states and must not invent further spellings.
#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum RunState {
    /// Admitted; no checkout exists yet.
    Admitted,
    /// The checkout is being materialized.
    Materializing,
    /// The validator is running.
    Running,
    /// Evidence is being extracted from the checkout.
    Extracting,
    /// The terminal record is being written; terminal at write.
    Recording,
    /// Terminal: tests passed.
    Passed,
    /// Terminal: tests failed or a CPU-budget breach timed out.
    Failed,
    /// Terminal: no result was produced; never evidence of green.
    CouldNotRun,
    /// Terminal at admission: refused, no checkout, no failure row.
    Refused,
    /// Legacy terminal spelling (pre-lifecycle writers).
    Finished,
    /// Legacy stopped spelling (pre-lifecycle writers).
    Stopped,
    /// Legacy reaped spelling (pre-lifecycle writers).
    Stale,
}

/// Cleanup track of one run, orthogonal to the verdict state.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum CleanupState {
    /// Deletion has not been attempted or is not owed.
    #[default]
    None,
    /// Deletion was attempted and is incomplete; journaled debt.
    Pending,
    /// The checkout is deleted (or never existed).
    Done,
}

/// One run handle record (`local-ci-hub/run/v1`).
#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
pub struct RunHandle {
    /// Schema tag; always `local-ci-hub/run/v1`.
    pub schema: String,
    /// Run identity, unique per admitted run.
    pub run_id: String,
    /// Exact commit the run validates (or names, when refused).
    pub commit: String,
    /// Cell or profile name, empty when unscoped.
    #[serde(default)]
    pub cell: String,
    /// Verdict lifecycle state.
    pub state: RunState,
    /// Cleanup track; verdicts never wait on it.
    #[serde(default)]
    pub cleanup: CleanupState,
    /// Ledger outcome spelling once terminal, when recorded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcome: Option<String>,
    /// Component exit code once terminal, when recorded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i64>,
    /// Start time as an ISO-8601 timestamp, when recorded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at: Option<String>,
    /// Host-limits file consulted for admission, when recorded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host_limits_path: Option<String>,
}

/// Schemas this module generates, by file stem.
pub fn generated_schemas() -> Vec<(&'static str, schemars::Schema)> {
    vec![("local-ci-hub-run-v1", schemars::schema_for!(RunHandle))]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wire_spellings_are_stable() {
        let handle = RunHandle {
            schema: SCHEMA.to_string(),
            run_id: "run-1".to_string(),
            commit: "a".repeat(40),
            cell: String::new(),
            state: RunState::CouldNotRun,
            cleanup: CleanupState::Pending,
            outcome: Some("no_result".to_string()),
            exit_code: Some(75),
            started_at: None,
            host_limits_path: None,
        };
        let text = serde_json::to_string(&handle).expect("serializes");
        assert!(text.contains("\"state\":\"could-not-run\""), "{text}");
        assert!(text.contains("\"cleanup\":\"pending\""), "{text}");
        let back: RunHandle = serde_json::from_str(&text).expect("parses");
        assert_eq!(back, handle);
    }
}
