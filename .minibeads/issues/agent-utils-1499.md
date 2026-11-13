---
title: 'tests-touch-live-registry: tests resolve the live harness registry by ancestor walk and write to it'
status: closed
priority: 0
issue_type: bug
labels:
- agentctl
created_at: 2026-11-13T00:00:00+00:00
updated_at: 2026-11-13T00:00:00+00:00
---

# Description

[watermelon-full-preview] P0: tests are mutating a live agentctl registry. A kernel rename trace (2026-11-13) shows a pytest worker process running the agentctl test suites calling renameat on a live harness registry's top-level `restore/` directory, moving it into `agents/restore`. It happened at least five times in one day.

Lane worktrees live under the harness directory, and agentctl's default registry is the nearest `.agentctl` in the working directory's ancestors (#1169 slot-registry-discovery). Any test, fixture or in-process call that does not pass an explicit registry therefore resolves to the harness's real `.agentctl` and runs write-path code against it — here the registry layout auto-migrate in a worktree whose base predates the reserved-names fix. The move itself is fixed on main (the reserved-names change: `restore/` is reserved), but the isolation hole remains: any write a test makes can land in the live registry.

# Design

- Find every test (Python and Rust, including cross/differential adapters) that can resolve a registry by the ancestor walk or by the bare `.agentctl` default, using a canary: run the agentctl test suites with the working directory inside a temp tree that has a sentinel `.agentctl` above it, and list what touches it. Fix each one to use its tmp registry explicitly.
- Defence in depth in the test harness: `scripts/run_pytest_shard.py`, the validate/gate step environment and the Rust test helpers set `AGENTCTL_REGISTRY` to a per-run scratch directory and `AGENTCTL_LAYOUT_AUTO_MIGRATE=0`, unless a test sets its own.
- A gating check (fast, no wall clock) that fails if a test run writes anything outside its tmp dirs into an ancestor `.agentctl`: the sentinel registry above the run's working directory must be byte-identical before and after.
- Separately consider whether agentctl's ancestor walk should stop at a git worktree boundary or refuse a registry that is not in the same repository root as the working directory. That is a behaviour change for real users and needs the owner's OK before it lands; it is proposed, not implemented, here.

# Acceptance Criteria

- The canary run (sentinel `.agentctl` above the test working directory, outside the harness) lists every test that touches the sentinel, and each listed test is fixed to use its tmp registry explicitly.
- The pytest shard harness, the gate/validate step environment and the Rust test helpers run agentctl tests with `AGENTCTL_REGISTRY` pointing at a per-run scratch directory and `AGENTCTL_LAYOUT_AUTO_MIGRATE=0`, unless a test sets its own registry.
- A fast gating check with no wall-clock assertion fails when a test run writes into an ancestor `.agentctl` (the sentinel is byte-identical before and after the run).
- No test run from a worktree under a harness directory writes to the harness's live registry.

# Notes

The restore drill is not waivable. No wall-clock assertions (owner rule 346).

Considered and rejected (coordb, 2026-11-13): (a) stopping agentctl's ancestor walk at a git worktree boundary, and (b) refusing a registry outside the working directory's repository root. Both would break #1169 slot-registry-discovery, which depends on the lexical walk crossing a wrkslots slot's worktree boundary to reach the harness registry above it. Discovery stays as it is; test isolation plus the sentinel gating check close the hole that actually bit us. No agentctl behaviour change lands here.

## Resolution

[watermelon-full-preview] Canary in a temp tree outside the harness (sentinel `.agentctl` with `restore/reports/` and a private legacy agent) reproduced the touch: a bare write-path `cli.main(["revive", "--all"])` resolved the sentinel by ancestor walk and moved its legacy agent into `agents/`. The bare revive calls in `test_agentctl_lifecycle.py` now pass their tmp registry explicitly; the remaining Python, Rust and cross suites already pass explicit registries (Rust audit: every write-path invocation names `--registry`). Defence in depth: `scripts/run_pytest_shard.py` (guarded worker environment and plugin configure), `scripts/run_component_tests.py`, `scripts/validate.py` and `scripts/gate.py` child environments set `AGENTCTL_REGISTRY` to a per-run scratch directory and `AGENTCTL_LAYOUT_AUTO_MIGRATE=0` unless a registry was already chosen, and the cross adapters drop any inherited `AGENTCTL_REGISTRY` with auto-migrate off. The gating check `py/tests/test_agentctl_registry_isolation.py` (no wall clock) runs a bare write through the shard harness under a sentinel and requires the sentinel byte-identical, and covers the Rust/gate and cross surfaces through their shared child/adapters environments. Item 4 stays (c): discovery unchanged per coordb; see Notes.
