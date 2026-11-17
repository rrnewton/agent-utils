---
title: 'local-ci-hub-run-handle-format: register the run handle states, fix the editions mirror lag, add the skill pointer'
status: closed
priority: 2
issue_type: task
labels:
- local-ci-hub
created_at: 2026-11-17T12:40:45.000000000+00:00
updated_at: 2026-11-17T12:40:45.000000000+00:00
---

# Description

Step 2a of the local-ci-hub work: register the run handle format
before the checkout lifecycle grows new states, correct the
editions record for the existing Rust read core, and give the tool
its thin skill pointer. Registration only; no lifecycle behaviour
changes in this step.

# Design

- One Rust type for the run handle in the existing Rust crate,
  generating the checked-in JSON Schema; the Python writer stays
  the primary edition and validates against that schema in tests.
- The handle state vocabulary covers the lifecycle and cleanup
  states the next step needs, so that step adds no version bump.
- The editions entry stops calling the Rust mirror absent: the
  crate exists as a read core and is recorded as paused, with the
  generated table regenerated.
- The skill is a thin pointer to the CLI help, quickstart and
  userguide, following the suite pattern.

# Acceptance Criteria

- The formats registry names the run handle, its Rust type, its
  Python writer and its generated schema; the schema checker
  regenerates byte-identically.
- Python-produced sample handles validate against the schema,
  including the new lifecycle and cleanup states.
- The editions check passes with the corrected mirror state.
- The skill pointer exists and names only the CLI surfaces.

# Notes

Sweeper authority question for the coordinator (10 lines):
An attended sweeper runs at run start under the caller's own
authority and needs no new identity. An unattended timer sweeper
has no coordinator ancestor to name as its authority, and faking
one would let any timer delete checkouts host-wide. The options
are a named service identity the sweeper legitimately acts as,
or an equivalent owner-approved authority path in the slot tool.
Until one is agreed, the timer template ships documented but
refuses to run without a configured service identity. The
coordinator takes this question to the consumer and slot owner.

Receipt-field contract is drafted separately and agreed before
any receipt code changes; it is not part of this step.

## Resolution

[watermelon-full-preview] Landed on main at 18009725. The run
handle format is registered with its generated schema, the
editions mirror state is corrected, and the skill pointer exists.
The gate was red only on an unrelated browser step, whose solo
rerun passed. The receipt contract and sweeper authority answers
arrived during this step and are handled as follow-ups; no
lifecycle or receipt code changed here.
