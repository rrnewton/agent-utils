---
title: 'ci-failure-masking: the first failure in a validation run hides every other failure'
status: open
priority: 1
issue_type: bug
labels:
- validation
created_at: 2026-10-19T09:35:54.849757746+00:00
updated_at: 2026-10-19T09:35:54.849757746+00:00
---

# Description

[opus 5.5] `scripts/validate.py` runs the validation graph eagerly and offers no `--keep-going`, although dagrun has one, so the first red step aborts every independent step and they are reported only as "not launched". Inside `vibe-talk.test.unit`, `cargo test` also stops at the first failing test binary.

Observed:
- a stale partition pin and a strict-typing error stayed hidden behind another failure until a later push (2026-10-15);
- on 2026-10-18 a message-cache test failure hid a separate vibe-talk test failure on main (the second binary never ran);
- the nightly aborted wrkslots shard 2 after 315 s because shard 1 had failed, and reported 131 steps as not launched.

Fix: add `validate.py --keep-going` (passed to dagrun; steps that depend on a failure are still skipped); use it in repository-ci and the nightly; run the vibe-talk Rust suites with `--no-fail-fast`. Local `make validate` keeps its eager default for fast feedback.

Acceptance: a run with one failing step still finishes every independent step, and its summary lists every failure.
