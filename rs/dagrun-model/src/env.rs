//! Shared environment-variable names for the DAG runner and its consumers.

/// Cgroup path delegated to a nested run.
pub const DELEGATED_CGROUP_ENV: &str = "DAGRUN_DELEGATED_CGROUP";
/// Alias spelling of [`DELEGATED_CGROUP_ENV`].
pub const DAGRUN_DELEGATED_CGROUP: &str = DELEGATED_CGROUP_ENV;

/// Marks a nested run that runs unboxed.
pub const DELEGATED_UNBOXED_ENV: &str = "DAGRUN_DELEGATED_UNBOXED";
/// Alias spelling of [`DELEGATED_UNBOXED_ENV`].
pub const DELEGATED_UNBOXED: &str = DELEGATED_UNBOXED_ENV;

/// Direct cgroup path for a nested run.
pub const DIRECT_CGROUP_ENV: &str = "DAGRUN_DIRECT_CGROUP";
/// Alias spelling of [`DIRECT_CGROUP_ENV`].
pub const DIRECT_CGROUP: &str = DIRECT_CGROUP_ENV;

/// Marks a process already inside the runner's scope.
pub const IN_SCOPE_ENV: &str = "DAGRUN_IN_SCOPE";
/// Alias spelling of [`IN_SCOPE_ENV`].
pub const IN_SCOPE: &str = IN_SCOPE_ENV;

/// Names the scope unit a nested run belongs to.
pub const SCOPE_UNIT_ENV: &str = "DAGRUN_SCOPE_UNIT";
/// Alias spelling of [`SCOPE_UNIT_ENV`].
pub const SCOPE_UNIT: &str = SCOPE_UNIT_ENV;

/// Forces a scope attempt even when one would normally be skipped.
pub const FORCE_ATTEMPT_ENV: &str = "DAGRUN_FORCE_SCOPE_ATTEMPT";
/// Alias spelling of [`FORCE_ATTEMPT_ENV`].
pub const FORCE_SCOPE_ATTEMPT: &str = FORCE_ATTEMPT_ENV;

/// The name a step reads to find where ready artifacts are announced.
pub const ARTIFACT_READY_DIR_ENV: &str = "DAGRUN_ARTIFACT_READY_DIR";
/// Alias spelling of [`ARTIFACT_READY_DIR_ENV`].
pub const ARTIFACT_READY_DIR: &str = ARTIFACT_READY_DIR_ENV;

/// Monotonic step-start timestamp handed to a nested run.
pub const STEP_STARTED_MONOTONIC_NS_ENV: &str = "DAGRUN_STEP_STARTED_MONOTONIC_NS";

/// Tag of the enclosing run handed to a nested run.
pub const OUTER_RUN_ENV: &str = "DAGRUN_OUTER_RUN";
