---
title: 'herdr-run-fresh-pane-first-render-grace: the first command into a freshly opened pane waits for its first render instead of failing PANEBUSY'
status: open
priority: 2
issue_type: bug
labels:
- herdr-run
created_at: 2026-11-25T23:08:00-05:00
updated_at: 2026-11-25T23:08:00-05:00
---

# Description

Reported by a consuming team: twice in one evening, the first herdr-run call into a FRESH pane returned rc 75 PANEBUSY before running anything: "pane looked idle but not for 2 consecutive checks within 0s ... prompt=abstain (pane has no rendered output yet)". The audit shows ADMITTED, then PANEBUSY, and no RAN record; a retry 15 s later ran fine. Cause: the default ready_timeout_seconds is 0 (fail fast on a busy pane, by design), so wait_ready takes only its two readings 250 ms apart; a pane herdr-run has just opened has not rendered its shell prompt yet, so the readiness verdict abstains on the first reading and the consecutive count never reaches 2.

# Design

- A pane herdr-run opened in this same invocation (or whose run record shows no completed run yet) gets a bounded first-render grace before the readiness verdict: wait on the event or condition "the pane has rendered output" (polling the pane's screen, not a wall-clock sleep), up to a small configurable bound (for example first_render_grace_seconds, default a few seconds), then apply the normal two-consecutive-checks rule.
- Reused panes keep today's fail-fast behaviour with ready_timeout_seconds 0.
- Rust and Python editions change together, with the shared readiness_cases test data.

# Acceptance Criteria

- A test where the fake pane renders nothing on the first readings and its prompt after a short delay: the first command into a freshly opened pane runs (no PANEBUSY), with ready_timeout_seconds 0.
- A reused busy pane still fails fast with PANEBUSY at ready_timeout_seconds 0.
- No wall-clock budget in the test (condition-based waits only).

# Notes

Workaround until it lands: pass --wait-ready N for a first call, or set ready_timeout_seconds in the herdr-run config. Related: the idle-tab TTL sweep closes command tabs between runs, which makes fresh panes more common.
