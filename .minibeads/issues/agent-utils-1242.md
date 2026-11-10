---
title: 'testrun-core-crate: extract the test-run core from dagrun into its own crate'
status: closed
priority: 3
issue_type: task
created_at: 2026-11-10T18:31:04+00:00
updated_at: 2026-11-10T19:54:22+00:00
closed_at: 2026-11-10T19:54:22+00:00
---

# Description

[opus 5.5] A consumer's end-to-end test harness wants dagrun's process-subtree CPU accounting and test-result handling on a Buck build path, without linking all of dagrun. Today those live inside the dagrun crate: `rs/dagrun/src/proccpu.rs` (process-group and subtree CPU time, with the cgroup and procfs sources), the parts of `rs/dagrun/src/procstat.rs` it uses, `rs/dagrun/src/test_results.rs`, and one environment-variable constant they share.

# Design

- New crate `rs/testrun-core` holding proccpu.rs, the procstat items it needs, test_results.rs and the shared constant, with a small dependency set (std plus whatever those modules already use; no dagrun scheduler, DAG or CLI code).
- dagrun depends on testrun-core and re-exports the old module paths, so every existing `dagrun::proccpu`, `dagrun::test_results` and `dagrun::procstat` user keeps compiling unchanged (backward compatible).
- Move the module tests and fixtures (`rs/dagrun/tests/fixtures/proccpu-*`) with the code; keep dagrun's integration tests passing.
- Consumer requirements (forwarded by the lead, 2026-11-10); the consumer vendors the crate byte-identical at a pinned revision, as it already does for dagrun:
  - No YAML dependency at all, not even optional or behind a default feature: no serde_norway, no serde_yaml. Dependencies are serde and serde_json only, plus libc if the procstat items need it.
  - No Buck target in this repository; plain Cargo is enough.
  - No build.rs, and no proc-macros beyond serde_derive.
  - Minimal features: the default covers proccpu, test_results and the shared constant, and nothing else.
  - A check in validation (for example over `cargo tree -p testrun-core`) that fails if any YAML crate, build script or extra proc-macro enters the crate's dependency tree.
- Package docs must be standalone (no sibling-crate or edition names in the crate's public docs).

# Acceptance Criteria

- `cargo build` and the full Rust validation pass with dagrun using the new crate; no public path of dagrun changes.
- testrun-core builds on its own with no dagrun dependency.
- The dependency-tree check runs in validation and passes: no YAML crate, no build script, no proc-macro other than serde_derive.

# Notes

Lead request 2026-11-10 (normal priority, about half a day plus one gate). Assign to a fresh lane on devbig030 after current #624 and #1201 work.

## Resolution

[opus 5.5] Delivered in the commit that closes this issue.

- New crate `rs/testrun-core` (library `testrun_core`, 0.1.0) holds `proccpu`, `test_results`, `procstat::status_field` (the one procstat item proccpu needs) and `TEST_COUNTS_PATH_ENV`, the environment-variable constant a test runner writes its results to. The module tests and the `proccpu-*` fixtures moved with the code; the Python tests read the fixtures from their new place.
- dagrun depends on it (a versioned path dependency, so dagrun still packages) and re-exports the old paths: `dagrun::proccpu`, `dagrun::test_results`, `dagrun::attribution::TEST_COUNTS_PATH_ENV` and the existing root re-exports. dagrun's private procstat re-uses `status_field`. No public dagrun path changed. The one proccpu test that drives dagrun's scheduler moved to dagrun (`proccpu_fallback_tests`). It watches the new public `proccpu::live_registrations()` rather than the registry's private fields. A scheduler test that borrowed the proccpu helper fixture now builds its own small C helper.
- Dependencies: serde (no derive), serde_json and libc, default features only; no build script, no proc-macro, no features, no YAML. The resolved closure is testrun-core, serde, serde_core, serde_json, libc, itoa, memchr and zmij.
- Validation: the new step `rust.dagrun.testrun-core-deps` (scripts/check_testrun_core_deps.py, with `--self-test`) resolves the tree with `cargo tree -p testrun-core -e normal,build` and fails on any crate outside that allowlist, any YAML-named crate, any proc-macro other than serde_derive, a build script outside the base crates that already ship one (serde, serde_core, serde_json, libc, zmij), or a feature on testrun-core. Enabling serde's derive feature makes it fail, as intended. Reading of "no build.rs" (accepted by the coordinator 2026-11-10): a closure with zero build scripts is impossible with the dependencies this issue allows, since serde, serde_core, serde_json, libc and serde_json's zmij each ship one. So the check enforces no build script in testrun-core itself and none outside those base crates, and its failure message states this rule. testrun-core belongs to the dagrun component (`validation/components.json`), so any change to it selects every dagrun check. `rust.dagrun.test` now also runs `-p testrun-core`, and the dagrun package step packages and verifies testrun-core first (check_rust_packages registry and local package patch).
- No Buck target, as requested; plain Cargo.
