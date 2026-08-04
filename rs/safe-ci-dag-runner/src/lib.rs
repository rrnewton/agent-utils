//! safe-ci-dag-runner library.
//!
//! Load a DAG of CI/build steps from JSON, model it, size it (memory-aware `-j`), visualize
//! it (Graphviz DOT / ASCII), and RUN it concurrently with dependency + scarce-resource
//! gating and eager-exit on first failure.
//!
//! This Rust build reproduces the OBSERVABLE behavior of the Python reference
//! (`py/safe_ci_dag_runner`) for the scheduling core, proven identical by the randomized
//! differential test in `cross/differential.py`. It ALSO performs the real work: two-level
//! Linux cgroup-v2 per-step boxing ([`cgroup`]), zombie-free teardown, ambient-load sampling
//! ([`ambient`]), and CSV perf logging ([`perflog`]) — matching the Python build's behavior.
//! Cgroup boxing is environment-dependent, so the differential exercises the un-boxed core
//! (`--allow-cgroup-failure`) while a dedicated smoke test proves boxing caps memory.

pub mod ambient;
pub mod cgroup;
pub mod cli;
pub mod estimates;
pub mod io;
pub mod model;
pub mod perflog;
pub mod profile_enrich;
pub mod scheduler;
pub mod sizing;
pub mod summary;
pub mod sync;
pub mod viz;

pub use ambient::{
    ambient_bucket, attribute_external_cores, capture_ambient_snapshot, AmbientBucket,
    AmbientSnapshot,
};
pub use cgroup::{install_scope_teardown, reexec_in_scope, CgroupManager, Cgroups};
pub use estimates::{
    allocate_widths, apply_plan_to_config, bucketize_rows, build_plan, feedback_identity,
    load_step_samples, load_step_speedups, plan_to_json, plan_to_text, sample_from_row,
    step_samples_from_buckets, step_speedups_from_buckets, Allocation, BucketKey, Plan, PlanEntry,
    Planner, Sample, SpeedupLevel, StepSamples, StepSpeedup, DEFAULT_MIN_SAMPLES,
};
pub use io::{
    dag_from_json, dag_from_value, dag_from_yaml, dag_to_json, dag_to_yaml, DagJsonError,
};
pub use model::{
    command_with_inner_jobs, effective_jobs_flag, preferred_inner_jobs, render_jobs_flag,
    step_classification, step_failure_reason, DagConfig, ResourceHint, RunResult, Step, StepClass,
    StepOutcome, DEFAULT_JOBS_FLAG, DEFAULT_STEP_TIMEOUT,
};
pub use perflog::{append_step_profiles, PerfWindow};
pub use profile_enrich::{
    container_core_budget, resolve_effective_inner_jobs, step_enrichment_columns,
};
pub use scheduler::{run_dag, run_dag_boxed, run_dag_boxed_ordered};
pub use sizing::{
    jobs_footprint_bytes, jobs_for_budget, mem_available_bytes, parse_size,
    schedulable_peak_mem_bytes, schedulable_peak_mem_bytes_widths, step_mem_cap_bytes,
    step_mem_cap_for_inner_jobs, transitive_deps,
};
pub use viz::{to_ascii, to_dot};

/// Program name (matches the Python build so `--version` output is byte-identical).
pub const PROG: &str = "safe-ci-dag-runner";

/// Crate version, sourced from Cargo at build time.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Machine-readable manifest of the enforcement guards THIS engine implements, emitted verbatim by
/// the `capabilities` subcommand. Each engine declares its own truth; the py-vs-rs differential
/// asserts the two manifests are BYTE-IDENTICAL, so any enforcement guard present in one build but
/// missing from the other is a cross-check failure (the recurrence guard for the historical
/// Rust-vs-Python `cpu_timeout` gap). Keys are sorted; values reflect real behavior:
///   cpu_affinity  opt-in --cores K: constrain the WHOLE run tree to K least-busy free cores
///                 (cgroup cpuset where delegated, else sched_setaffinity; verified + logged)
///   cpu_timeout   per-step user+system CPU budget (cgroup cpu.stat), reaped over budget
///   memory_max    per-step inner memory.max cap (kernel OOM-kills the step at its cap)
///   oom_detection failure attributed to OOM via cgroup memory.events oom_kill count
///   pids_guard    per-step PID/thread ceiling (plumbed in both, enforced in neither → false)
///   wall_timeout  per-step wall-clock ceiling (load-dependent; active with or without boxing)
/// The cgroup-dependent guards take effect only under boxing; the boxed smoke tests in each build
/// anchor these declarations to real behavior wherever a cgroup-v2 + systemd --user scope exists.
pub const ENFORCEMENT_CAPABILITIES: &str =
    "{\"cpu_affinity\":true,\"cpu_timeout\":true,\"memory_max\":true,\"oom_detection\":true,\"pids_guard\":false,\"wall_timeout\":true}";
