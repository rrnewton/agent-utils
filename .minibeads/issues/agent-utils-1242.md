---
title: 'testrun-core-crate: extract the test-run core from dagrun into its own crate'
status: open
priority: 3
issue_type: task
created_at: 2026-11-10T18:31:04+00:00
updated_at: 2026-11-10T18:31:04+00:00
---

# Description

[opus 5.5] A consumer's end-to-end test harness wants dagrun's process-subtree CPU accounting and test-result handling on a Buck build path, without linking all of dagrun. Today those live inside the dagrun crate: `rs/dagrun/src/proccpu.rs` (process-group and subtree CPU time, with the cgroup and procfs sources), the parts of `rs/dagrun/src/procstat.rs` it uses, `rs/dagrun/src/test_results.rs`, and one environment-variable constant they share.

# Design

- New crate `rs/testrun-core` holding proccpu.rs, the procstat items it needs, test_results.rs and the shared constant, with a small dependency set (std plus whatever those modules already use; no dagrun scheduler, DAG or CLI code).
- dagrun depends on testrun-core and re-exports the old module paths, so every existing `dagrun::proccpu`, `dagrun::test_results` and `dagrun::procstat` user keeps compiling unchanged (backward compatible).
- Move the module tests and fixtures (`rs/dagrun/tests/fixtures/proccpu-*`) with the code; keep dagrun's integration tests passing.
- Buck-side requirements from the consumer (target layout, features, dependency limits) are forwarded by the lead before implementation starts; wait for them.
- Package docs must be standalone (no sibling-crate or edition names in the crate's public docs).

# Acceptance Criteria

- `cargo build` and the full Rust validation pass with dagrun using the new crate; no public path of dagrun changes.
- testrun-core builds on its own with no dagrun dependency.
- The consumer's Buck-side needs, once forwarded, are met or explicitly listed as follow-ups.

# Notes

Lead request 2026-11-10 (normal priority, about half a day plus one gate). Assign to a fresh lane on devbig030 after current #624 and #1201 work.
