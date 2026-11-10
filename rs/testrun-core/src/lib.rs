//! Building blocks for a test harness that measures and records the tests it runs.
//!
//! - [`proccpu`]: best-effort CPU time of a process group, or of a process subtree,
//!   from Linux procfs, for commands that are not boxed in their own cgroup.
//! - [`test_results`]: the structured per-step test-result report a controlled test
//!   runner writes, with its strict reader and atomic writer.
//! - [`procstat`]: byte-safe field lookup in `/proc/PID/status` records.
//! - [`TEST_COUNTS_PATH_ENV`]: the environment variable naming where a test runner
//!   writes its report.

pub mod proccpu;
pub mod procstat;
pub mod test_results;

pub use test_results::TEST_COUNTS_PATH_ENV;
