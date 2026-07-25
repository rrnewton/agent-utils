//! safe-ci-dag-runner library.
//!
//! Load a DAG of CI/build steps from JSON, model it, size it (memory-aware `-j`), visualize
//! it (Graphviz DOT / ASCII), and RUN it concurrently with dependency + scarce-resource
//! gating and eager-exit on first failure.
//!
//! This Rust build reproduces the OBSERVABLE behavior of the Python reference
//! (`py/safe_ci_dag_runner`) for that core, proven identical by the randomized differential
//! test in `cross/differential.py`. Scope note for 0.1: the Rust `run` performs NO per-step
//! cgroup boxing and NO perf logging (matching Python's DEFAULT, where boxing is the opt-in
//! `--cgroups` path); those Linux-only modules stay Python-only for now.

pub mod model;
pub mod sizing;

pub use model::{
    preferred_inner_jobs, step_classification, step_failure_reason, DagConfig, ResourceHint,
    RunResult, Step, StepClass, StepOutcome, DEFAULT_STEP_TIMEOUT,
};
pub use sizing::{
    jobs_footprint_bytes, jobs_for_budget, mem_available_bytes, parse_size,
    schedulable_peak_mem_bytes, step_mem_cap_bytes, step_mem_cap_for_inner_jobs, transitive_deps,
};

/// Program name (matches the Python build so `--version` output is byte-identical).
pub const PROG: &str = "safe-ci-dag-runner";

/// Crate version, sourced from Cargo at build time.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
