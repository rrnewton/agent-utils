//! DAG model, strict JSON IO, pure helpers, and shared env names.
//!
//! This crate holds the pieces that a downstream build needs
//! without the runner: the [`model`] types, the strict JSON reader and
//! byte-exact writer in [`io`], pure helpers, and the [`env`] names. It does not depend on a scheduler, cgroup, CLI runtime, or any YAML parser.

pub mod ascii;
pub mod check;
pub mod clock;
pub mod env;
pub mod io;
pub mod model;
pub mod select;

pub use io::{dag_from_json, dag_from_value, dag_to_json, DagJsonError};
pub use model::{
    ArtifactDep, CmdType, DagConfig, DagManifest, PublishedArtifact, ResourceHint, ResultManifest,
    Step, StepClass, StructuredTestResultsManifest,
};

pub use ascii::to_ascii_groups;
pub use check::steps_violating_run_timeout;
pub use clock::monotonic_now_ns;
pub use select::{
    apply_passthrough_args, select_steps_by_labels, select_steps_by_tags, ARGS_PLACEHOLDER,
};
