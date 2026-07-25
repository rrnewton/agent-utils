//! safe-ci-dag-runner library.
//!
//! Two-level cgroup boxing, memory-aware concurrency, and always-on CPU/mem/ambient-load
//! logging for a DAG of CI/build steps. The full API is being ported from a mature
//! reference implementation and kept behaviorally identical to the Python build by
//! randomized differential tests in CI.

/// Program name (matches the Python build so `--version` output is byte-identical).
pub const PROG: &str = "safe-ci-dag-runner";

/// Crate version, sourced from Cargo at build time.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Help text shown for `--help`.
pub fn help_text() -> String {
    format!(
        "{PROG} {VERSION}\n\
         Run a DAG of CI/build steps under nested cgroup CPU/memory boxing.\n\n\
         USAGE:\n    {PROG} [--version] [--help]\n"
    )
}
