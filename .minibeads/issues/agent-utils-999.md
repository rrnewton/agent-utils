---
title: 'pr-landing-planner-silent-hang: the advisory planner ran ~30 min with no plan, no progress and no error'
status: open
priority: 3
issue_type: bug
created_at: 2026-11-07T03:10:00.000000000+00:00
updated_at: 2026-11-07T03:20:00.000000000+00:00
---

# Description

A consumer ran the advisory `pr-landing-planner` on two pull requests on 2026-11-06. After about 30 minutes it still had no plan. Its stderr named only the Rust engine and its cache, its JSON output stayed empty, and it printed no error and no progress, so the operator killed it. Nothing said which phase it was in or what it was waiting on.

# Design

- Print progress to stderr: the phase (fetch, overlap analysis, ordering, ...) and counts (PRs, files, pairs done of total) at least every few seconds while working.
- A bounded default budget (wall time, with a flag to raise it). On expiry, exit non-zero with a named reason: which phase and which input (PR, file pair, remote call) it was on, plus any partial result marked partial.
- Any network or subprocess call gets its own timeout, named in the error when it fires.
- Batch gh reads (one GraphQL query or one `pr list`) instead of one call per item, and when the gh command is slow to return because a pacing wrapper sleeps, say so ("waiting for gh: N calls made, M s waited").
- Keep this in mind for #895 pr-landing-planner-retirement: the folded `merge-queue plan --advisory` should keep the same progress and budget behaviour.

# Acceptance Criteria

- A test with a stub input that never completes a phase: the planner prints phase progress, then exits within the budget with a reason naming the phase and input.
- A normal two-PR run prints phase and count lines to stderr and the plan JSON to stdout, unchanged.

# Notes

Reported 2026-11-06 by the lead coordinator from a consumer. P3: parked until a lane has slack. To reproduce, ask the lead coordinator for the consumer's exact command and inputs.

## Reproduction (2026-11-06)

- pr-landing-planner 0.1.0, engine=rust, source=50decc5939c5, binary=86b681a87295.
- Command: `pr-landing-planner plan --repo <owner>/<repo> --base main --git-dir <repo> --prs <two PRs> --net-wrapper with-proxy --gh-cmd <paced gh wrapper> --format json --no-archive`, with no landing-context, flaky-signatures or fixture inputs.
- stdout stayed 0 bytes; stderr had only the two engine/cache banner lines; killed after about 30 minutes. The output and error files are kept on the lead coordinator's host.
- Likely cause, check first: the `--gh-cmd` was a host-wide paced gh wrapper that allows one call per 30 s (burst 1), counts reads as well as writes, and sleeps for its slot. A planner that makes dozens of gh calls then looks hung for tens of minutes. Hence the batching and "waiting for gh" asks in Design.
