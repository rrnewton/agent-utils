---
title: "wrkslots-census-verdict-parity: bounded census exhaustion must give the same audit verdict in both editions"
status: open
priority: 1
issue_type: bug
created_at: 2026-11-11T18:30:00.000000000+00:00
updated_at: 2026-11-11T18:30:00.000000000+00:00
---

# Description

Under gate load, `test_audit_census_resume_state_matches_python` in
`py/wrkslots/tests/test_audit_state_rust.py` diverged: with the same small
census work limit, the Rust audit reported `UNKNOWN` for the fixture slot
while the Python audit reported an attention verdict for the same state.
The bounded cache census is allowed to stop early; stopping early is not
allowed to change the verdict differently in the two editions. One edition
treats budget exhaustion during the census as making the row's verdict
unknown, and the other keeps the verdict it could already prove from the
non-census evidence. A coordinator reading `audit` to decide what is safe
to reclaim must get the same answer from either edition on the same
registry, loaded host or not.

# Design

- Find where each edition folds a partial or exhausted census result into
  the audit row verdict, and make the rule one rule: census exhaustion may
  mark cache fields unknown (`cache_status`, `cache_error`, size fields)
  but must not by itself flip a verdict that other evidence already
  decided, in either direction.
- Keep the bounded census itself unchanged: the work limit, the resume
  state file, and the counters already agree across editions, as the rest
  of the resume-state differential shows.
- The differential must drive both editions into exhaustion on purpose
  (a work limit small enough that neither can finish) rather than rely on
  host load to create the condition.

# Acceptance Criteria

- With an intentionally exhausted census budget, both editions return the
  same verdict, the same `unknown_slots`, and the same top-level state for
  the resume fixture, in JSON and human output.
- A verdict already proven by liveness or holder evidence survives census
  exhaustion identically in both editions.
- `test_audit_census_resume_state_matches_python` passes repeatedly under
  load and its #1267 quarantine mark is removed.

# Notes

Filed at the lead coordinator's request after the #1214 gate evidence: the
divergence was first treated as a flaky test, but the verdict difference is
a product parity gap. Until this is fixed the resume differential is
quarantined under #1267 wrkslots-load-robust (owner reply 346). Audit
stays off Rust routing while this is open.
