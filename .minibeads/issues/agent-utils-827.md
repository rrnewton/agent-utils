---
title: 'gate-flake-triage: raise the full-gate pass rate by triaging steps that fail in full runs and pass alone'
status: open
priority: 1
issue_type: bug
assignee: mu-validate@devbig030
labels:
- validation
created_at: 2026-11-03T19:55:54.120106978+00:00
updated_at: 2026-11-03T19:55:54.120106978+00:00
---

# Description

[opus 5.5] Filed for the devbig014 coordinator (P1). On 2026-11-03 only 24 of 39 full gates passed (62%), counted from that day's gate logs since midnight ET, excluding reruns. Most failures were steps that passed when rerun alone, so lanes paid solo reruns or landed past a red by judgment. Full-gate failures that day: rust.agentctl.test 4; cross.agentctl.differential-2 3; python.agentctl.shard-0 3; vibe-talk.check.keyboard-browser 2; cross.herdr-run.differential 2 (#815); singles: rust.chat-subscription.slack, vibe-talk.check.thread-card-header-browser, vibe-talk.shots.screenshots, vibe-talk.check.desktop-layout-browser, python.agentctl.shard-1, rust-packages.rust.agentctl (and one genuine rustfmt miss). python.wrkslots.shard-3 OOMs whenever selected (#818).

# Design

For each repeat offender decide, with evidence: LOAD (memory or CPU contention inside the full dagrun: compare its profile and timing in failing runs with solo runs) or a REAL RACE (failing case names and order, reproduction under an artificial load). Fix load problems in the scheduler or the step's budget (owner rule: CPU budgets bite, wall clock about 8x, memory caps with headroom per #818); fix races in the test or the product, never with longer sleeps or by skipping. Record each verdict and fix in this issue. Follows #815 and #818.

# Acceptance Criteria

- The full-gate pass rate, measured the same way over a working day after the fixes, is at least 90%.
- Each step listed above has a recorded verdict (load or race) and either a fix or a reason it was left.
- No step is fixed by skipping it or by lengthening a sleep.
