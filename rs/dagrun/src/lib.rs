//! Build, validate, visualize, and execute resource-aware DAGs of CI steps.
//!
//! The runner provides dependency scheduling, memory-aware concurrency, Linux cgroup-v2
//! containment, teardown, host-load sampling, and performance logging.

// dagrun library.
//
// Load a DAG of CI/build steps from JSON, model it, size it (memory-aware active-step ceiling), visualize
// it (Graphviz DOT / ASCII), and RUN it concurrently with dependency + scarce-resource
// gating and eager-exit on first failure.
//
// This Rust build reproduces the OBSERVABLE behavior of the Python reference (`py/dagrun`)
// for the scheduling core, proven identical by the randomized differential test in
// `cross/differential.py`. It ALSO performs the real work: two-level Linux cgroup-v2
// per-step boxing ([`cgroup`]), zombie-free teardown, ambient-load sampling ([`ambient`]),
// and CSV perf logging ([`perflog`]) — matching the Python build's behavior. Cgroup boxing
// is environment-dependent, so the differential exercises the un-boxed core
// (`--allow-cgroup-failure`) while a dedicated smoke test proves boxing caps memory.

pub mod admission;
pub mod ambient;
pub mod attribution;
pub mod capabilities;
pub mod cgroup;
pub mod cli;
pub mod cpuset_allocator;
pub mod estimates;
pub mod io;
pub mod memory_feedback;
pub mod model;
pub mod perflog;
pub mod proccpu;
pub mod profile_enrich;
pub mod reservation;
pub mod resource_caps;
pub mod scheduler;
pub mod sizing;
pub mod summary;
pub mod sync;
pub mod viz;

pub use ambient::{
    ambient_bucket, attribute_external_cores, capture_ambient_snapshot, AmbientBucket,
    AmbientSnapshot,
};
pub use attribution::{
    bind_process_tests, culprit_columns, default_log_dir, mint_step_nonce, process_snapshot,
    recognize, Culprit, InFlightTest, ProcessObservation, RunEvidence, StepStream, TestEvent,
    TestTracker, LOG_DIR_ENV, NO_LOGS_ENV, REQUIRE_STRUCTURED_TEST_COUNTS_ENV, STEP_NONCE_ENV,
    TEST_COUNTS_PATH_ENV,
};
pub use capabilities::{enforcement_manifest, is_enforced, Capability, Lane, ENFORCEMENT_REGISTRY};
#[allow(deprecated)]
pub use cgroup::{
    aggregate_slice_cpu_jobs, aggregate_slice_max_cpus, attempt_scope_reexec,
    expected_outer_cpu_count, expected_scope_runtime_max_s, install_scope_teardown,
    observe_own_containment, promised_unit, run_containment, verify_scope_runtime_max,
    CgroupManager, Cgroups, ContainmentEvidence, ContainmentProof, RunContainment, ScopeAttempt,
    FORCE_ATTEMPT_ENV,
};
pub use estimates::{
    allocate_widths, apply_plan_to_config, bucketize_rows, build_plan, feedback_identity,
    load_step_samples, load_step_speedups, plan_to_json, plan_to_text, sample_from_row,
    step_samples_from_buckets, step_speedups_from_buckets, Allocation, BucketKey,
    InfeasibleAllocationError, Plan, PlanEntry, Planner, Sample, SpeedupLevel, StepSamples,
    StepSpeedup, DEFAULT_MIN_SAMPLES,
};
pub use io::{
    dag_from_json, dag_from_value, dag_from_yaml, dag_to_json, dag_to_yaml, DagJsonError,
};
pub use memory_feedback::{
    apply_memory_admissions, load_memory_admissions, memory_admission_from_rows,
    memory_admission_line, peak_observation_from_row, Censoring, MemoryAdmission, PeakObservation,
    DEFAULT_MARGIN_PCT, DEFAULT_MIN_UNCENSORED_SAMPLES,
};
pub use model::{
    command_with_inner_jobs, dag_config_carry_diff, effective_jobs_env, effective_jobs_flag,
    env_with_inner_jobs, preferred_inner_jobs, render_jobs_flag, resolve_jobs_env,
    resolved_wall_timeout, step_classification, step_failure_reason, step_width_is_resizable,
    undeclared_resource_demands, validate_jobs_env_config, write_domain_violations, DagConfig,
    ResourceHint, RunResult, Step, StepClass, StepOutcome, WriteDomainGuarantee, WriteDomainPolicy,
    DAG_CONFIG_FIELDS, DEFAULT_JOBS_FLAG, DEFAULT_STEP_TIMEOUT, JOBS_ENV_ENV,
    WALL_CPU_BACKSTOP_FACTOR,
};
pub use perflog::{append_step_profiles, PerfWindow};
pub use profile_enrich::{
    container_core_budget, resolve_effective_inner_jobs, step_enrichment_columns,
};
pub use reservation::{acquire as reserve_cores, held_cores, reclaim_dead, Reservation};
#[allow(deprecated)]
pub use scheduler::{
    cap_config_cpu_jobs, cap_config_max_cpus, nested_run_refusal, run_dag, run_dag_boxed,
    run_dag_boxed_deadline, run_dag_boxed_deadline_limited,
    run_dag_boxed_deadline_limited_with_cpu, run_dag_boxed_deadline_limited_with_resource_caps,
    run_dag_boxed_deadline_with_cpu, run_dag_boxed_limited, run_dag_boxed_ordered,
    run_dag_boxed_ordered_limited, run_dag_limited, start_run_cpu_budget,
    steps_violating_run_timeout, RunCpuBudget, OUTER_RUN_ENV,
};
pub use sizing::{
    box_mem_budget_bytes, cgroup_mem_max_bytes, jobs_footprint_bytes, jobs_for_budget,
    mem_available_bytes, parse_size, schedulable_peak_mem_bytes, schedulable_peak_mem_bytes_widths,
    step_mem_cap_bytes, step_mem_cap_for_inner_jobs, stress_copy_footprint_bytes, transitive_deps,
};
pub use viz::{to_ascii, to_dot};

/// Command name used in diagnostics and version output.
pub const PROG: &str = "dagrun";

/// Published package version.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
