//! Pluggable side-effect boundaries used by the tick engine.

/// The outcome of attempting to run a gate command.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GateResult {
    /// Process exit code, or `-1` when no process result exists.
    pub returncode: i32,
    /// Captured standard output.
    pub stdout: String,
    /// Whether the command actually ran to completion.
    pub ok: bool,
    /// Launch or timeout reason when `ok` is false.
    pub error: Option<String>,
}

impl GateResult {
    /// Construct a completed command result.
    pub fn completed(returncode: i32, stdout: impl Into<String>) -> Self {
        Self {
            returncode,
            stdout: stdout.into(),
            ok: true,
            error: None,
        }
    }

    /// Construct a command-execution failure.
    pub fn failed(error: impl Into<String>) -> Self {
        Self {
            returncode: -1,
            stdout: String::new(),
            ok: false,
            error: Some(error.into()),
        }
    }
}

/// Runs a reminder gate command.
pub trait GateRunner {
    /// Execute `cmd` and report its exit code and stdout.
    fn run(&self, cmd: &str) -> GateResult;
}

/// Measures the age of the newest file matching a glob.
pub trait FileAgeProbe {
    /// Return seconds since the newest match's mtime, or `None` when missing/unreadable.
    fn newest_age_secs(&self, pattern: &str, now: i64) -> Option<i64>;
}
