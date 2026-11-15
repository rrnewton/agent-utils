---
title: 'nightly-timeout-notifier: a timed-out scheduled full-contract run says so instead of staying silent'
status: open
priority: 1
issue_type: bug
external_ref: gh-134
created_at: 2026-11-15T17:47:04.000000000+00:00
updated_at: 2026-11-15T17:47:04.000000000+00:00
---

# Description

The scheduled full-contract job in .github/workflows/nightly-all.yml has had no successful run in its last 10 scheduled runs (2026-11-06 to 2026-11-15): three contract failures, then cancellations as the contract step reached the job's 90-minute timeout (most recently run 43711510791 on 2026-11-15, contract step 89 minutes). Because the notifier step is gated on failure(), timeout cancellations never comment on the tracking issue, so the loudest failure mode is now the silent one. Which validate steps failed on the finishing red runs still needs log-level attribution.

# Design

Decide the contract's size budget explicitly (shard the job, or raise/split the timeout with the reasoning recorded) and make the notifier also fire on cancellation of a scheduled run, naming timeout vs failure distinctly.

# Acceptance Criteria

- A scheduled run completes green inside its budget, or fails with named steps.
- A timeout/cancelled scheduled run produces an issue comment saying it timed out.
- No silent scheduled red nights.

# Notes

[watermelon-full-preview] Filed from GitHub #134 nightly-timeout-notifier (https://github.com/rrnewton/agent-utils/issues/134) during Request 098 triage. The GitHub issue is the auto-created series tracker; keep it open while nightly is red.
