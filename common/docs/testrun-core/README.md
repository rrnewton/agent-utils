# testrun-core

A test harness that runs commands wants two things from each run besides its exit
status: how much CPU time the command and its children used, and which tests passed,
failed or were skipped. This small Rust library provides both, with no runner,
scheduler or configuration language attached, so a harness can link it on any build
system that accepts plain Cargo crates.

## What it provides

- **`proccpu`: process-group CPU time from Linux procfs.** `ProcessGroupCpu::new(pgid)`
  binds the leader of a process group by pidfd, so a reader never follows a reused
  process ID. `seconds()` returns the group's own plus waited-child CPU time from one
  authenticated procfs observation. Readers share one snapshot, at most half a second
  old. A missing, ambiguous or resource-limited observation is an `Unavailable` error,
  never zero. Each scan has two budgets: one second of the scanning thread's CPU time,
  and a ten-second wall backstop for a scan that blocks without running.
  `subtree_cpu_seconds(pgid)` is a one-call form for callers that cannot keep a reader.
  This is best-effort accounting for commands that are not boxed in their own cgroup:
  a process that leaves the group, or activity between samples, can be missed.
- **`test_results`: structured per-run test reports.** A controlled test runner
  writes a JSON report of per-test results (`TestResults`, `TestResult`,
  `TestAttemptResult`) to the path named by the `TEST_COUNTS_PATH_ENV` environment
  variable. The reader is strict: it rejects duplicate keys, unknown schemas and
  malformed entries rather than guessing. The writer publishes the report
  atomically, with a recovery sibling (`structured_test_results_recovery_path`) for a
  report that was validated but could not be published.
- **`procstat`: `status_field`.** Byte-safe lookup of one field in a
  `/proc/PID/status` record, refusing duplicate or forged fields.

## Dependencies

`serde` (without its derive feature), `serde_json` and `libc`, with default
features only. The crate has no build script, no procedural macros and no features
of its own, and it does not parse YAML. Linux only: the CPU accounting reads procfs
and uses pidfds.

## Example

```rust,no_run
use testrun_core::proccpu::ProcessGroupCpu;

let child = std::process::Command::new("make")
    .arg("test")
    .spawn()
    .expect("spawn");
// Bind the group as soon as the command starts; the command must lead its own
// process group (for example, spawned with `process_group(0)`).
let reader = ProcessGroupCpu::new(child.id()).expect("bind the process group");
match reader.seconds() {
    Ok(seconds) => println!("used {seconds:.2}s of CPU so far"),
    Err(unavailable) => println!("CPU time unavailable: {unavailable}"),
}
```
