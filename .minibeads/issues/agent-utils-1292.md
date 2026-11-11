---
title: 'turnover-notify-empty-refusal: refuse to send a turnover phase with no phase or registry'
status: closed
priority: 1
issue_type: task
created_at: 2026-11-11T12:30:00+00:00
updated_at: 2026-11-11T12:30:00+00:00
closed_at: 2026-11-11T12:30:00+00:00
---

# Description

[watermelon-full-preview] Follow-up to #1220 services-turnover-timer-gaps.
During the devbig030 timer install verification, the turnover-notify
wrapper was run once by hand with only TURNOVER_REGISTRY set, and it
delivered a message with every field empty ("turnover : registry ...,
unit ,  -> , verdict") to that registry's coordinator. A notify wrapper
must never send a phase-less message: a mistaken invocation, a bare
shell, or any caller that did not come through turnover would spam a
coordinator with a blank report.

# Design

- `services turnover-notify` refuses before sending when
  TURNOVER_PHASE or TURNOVER_REGISTRY is empty or unset: a named error
  on stderr and a non-zero exit, and nothing is stored or sent.
- `--help` never sends (clap prints help before the command runs; the
  refusal keeps that true for every other early exit too).
- Routing verification stays read-only: recipient resolution is
  printable without a send (notify_recipient is a pure function and
  the unit tests exercise it).

# Acceptance Criteria

- A wrapper invocation with an empty or missing TURNOVER_PHASE, or an
  empty or missing TURNOVER_REGISTRY, exits non-zero, names the empty
  variable on stderr, and sends nothing.
- A test covers both variables, empty and unset, plus the complete
  environment that proceeds.
- The existing turnover suites stay green.

# Notes

The one blank message already delivered during install verification
was disclosed to the coordinator in the install report; no other
registry received one.

## Resolution

[watermelon-full-preview] Delivered with this closing commit.
`run_turnover_notify` validates the environment first through
`notify_env_problem`: a missing or empty TURNOVER_PHASE or
TURNOVER_REGISTRY returns a named error before any file is written or
send is attempted, which the CLI surfaces as a failure. Tests cover
both variables empty and unset, a blank phase with a registry set,
and a complete environment proceeding to recipient resolution.
Unverified: no live wrapper invocation was made to prove the refusal
(it is proven at the function boundary, deliberately, after the
install-day blank send).
