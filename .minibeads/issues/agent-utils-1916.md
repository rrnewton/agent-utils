---
title: 'agentctl-harness-wrapper-revive-liveness: agent-watch cannot notify a headless harness-wrapper coordinator'
status: closed
priority: 1
issue_type: bug
labels:
- agentctl
created_at: 2026-11-19T16:15:46-05:00
updated_at: 2026-11-19T16:15:46-05:00
---

# Description

[watermelon-full-preview] Defect reported by a downstream consumer
(found by its overnight checker; checked at agent-utils 74549a37): agent-watch
cannot notify a HEADLESS harness-wrapper coordinator. The notice check
in restore.rs dry-runs a revive of the target and treats it as alive
only when the plan says "skip ... alive". `revive_liveness`
(subagents/revive.rs) reads `custom_process_identity`, or
`harness_identity` only when the adapter is herdr/tmux AND
`anchor_rule == 2`. A harness-wrapper record (adapter
"harness-wrapper", mode headless, `custom_process_identity` null,
`anchor_rule` null) therefore gets liveness Unknown, and the notice
gate in restore.rs never sends ("blocked: recorded harness process
death is unverifiable", sent: false). In that record,
`harness_identity` (pid, starttime, dev, inode, boot) matched the
live process exactly.

# Design

`revive_liveness` uses `harness_identity` for harness-wrapper records
with the same full proof as the existing paths (pid + starttime +
boot, and exe dev/inode where recorded). A mismatched or dead
identity must read Dead or Unknown exactly as today, never Alive.
Check whether other adapters with null `anchor_rule` have the same
gap; fix any that are clearly the same. Keep Rust and Python editions
in parity. No new persisted field.

# Acceptance Criteria

- A harness-wrapper record with a matching live identity is Alive and
  agent-watch notices are sent.
- A stale pid, or a reused pid with a different starttime, is not
  Alive.
- At least one test drives a REAL child process (spawn, record its
  real /proc identity, kill it, re-check), and at least one test
  fails on the old code.
- Old tests pass on the new code with no edits to existing test
  doubles or assertions.

# Notes

[watermelon-full-preview] Lane mu-wrapperlive/1916. Reader skew: a
downstream reader pins an old reader (writer_ceiling 2); report any
new persisted field in a registry record before gating — prefer no
new persisted field.

## Resolution

[watermelon-full-preview] Landed on main as
744ac563ca6aba85b96ac9d0002f5d36747e7ef4 (gated as d06e0365, cold
review SAFE). `revive_liveness` (Rust) and `_old_process` (Python)
use the wrapper-recorded `harness_identity` for harness-wrapper
records under the unchanged full process proof; no new persisted
field. Where the verdict now reads Dead instead of Unknown, revive
still refuses headless records for lack of a pane identity, so
nothing launches twice. Tests: fake-client alive-to-skip, the
watch-level coordinator finding, dead/stale-pid/reused-pid guards,
and real-child tests in both editions; the alive tests fail on the
old code. Follow-ups: the Python loader still refuses the
harness-wrapper adapter (separate widening work).
