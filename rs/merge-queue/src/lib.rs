//! A local merge queue.
//!
//! Changes are submitted as ready to land; the queue orders them by priority, age and the exact
//! conflict graph, chunks them into batches, validates each batch's exact combined tree, and lands
//! only validated trees. Validation is a set of individual tests, never one boolean: every run
//! names its selection, focused runs leave a tracked obligation to widen to the complete suite,
//! and bisection reruns only the failed tests.

#![forbid(unsafe_code)]

pub mod bench;
pub mod bounded;
pub mod build_info;
pub mod cli;
pub mod decisions;
pub mod donor;
pub mod drain;
pub mod git;
pub mod github;
pub mod hooks;
pub mod intake;
pub mod interrupt;
pub mod known_flaky;
pub mod model;
pub mod notify;
pub mod ops;
pub mod pins;
pub mod plan;
pub mod precheck;
pub mod probe_cache;
pub mod recount;
pub mod run;
pub mod serve;
pub mod signal;
pub mod state;
pub mod suspect;
pub mod sync;
pub mod tests;
pub mod validate;

/// The complete user guide, also printed by `merge-queue userguide`.
pub const USER_GUIDE: &str = include_str!("embedded_userguide.md");
/// The getting-started guide, also printed by `merge-queue quickstart`.
pub const QUICKSTART: &str = include_str!("embedded_quickstart.md");
