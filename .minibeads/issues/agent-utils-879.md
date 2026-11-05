---
title: 'wrkslots-liveness-one-source: status and audit report the same owner liveness'
status: closed
priority: 2
issue_type: bug
assignee: mu-validate
created_at: 2026-11-05T09:32:58.478143832+00:00
updated_at: 2026-11-05T17:19:37.663878761+00:00
closed_at: 2026-11-05T17:19:37.663878430+00:00
---

# Description

`wrkslots status` reports a slot's owner liveness as "indeterminate", while `wrkslots audit` reports the same owner as "alive". Seen on two slots of a consumer project in a read-only registry triage on 2026-11-05.

Operators and automation read both commands, so their verdicts must not contradict each other.

# Design

- Find why they differ. Likely candidates: different liveness providers, a cheap path in status that skips a proof audit performs, a different PID namespace or cgroup view, or a timeout.
- Route both through one liveness function with the same inputs.
- If status deliberately takes a cheaper path, it must not say "indeterminate" where the full proof would say alive. It reports "not checked (run audit)", or runs the same proof.

# Acceptance Criteria

- One liveness source decides the owner state for both commands.
- A test with a live owner (a real child process with a registered generation) gets "alive" from both `status` and `audit`.
- A test with an unverifiable owner gets the same non-alive verdict from both.

# Notes

From the lead on 2026-11-05 (P2). Assigned to mu-validate after #878.
