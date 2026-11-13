//! Shared environment-variable names for the DAG runner and its consumers.

/// The name a step reads to find where ready artifacts are announced.
pub const ARTIFACT_READY_DIR_ENV: &str = "DAGRUN_ARTIFACT_READY_DIR";
