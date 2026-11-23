---
title: 'agentctl-watch-pane-census-budget: the watch pane census hits its budget every pass on a busy registry'
status: open
priority: 3
issue_type: bug
labels:
- agentctl
created_at: 2026-11-23T08:40:00-05:00
updated_at: 2026-11-23T08:40:00-05:00
---

# Description

With 25-plus lanes open, agentctl watch reports "hygiene census: pane census budget reached; unprobed panes were kept, not closed" on nearly every pass for a whole night. Nothing is closed (it fails safe), but the panes it never reaches are never checked, and the repeated notice is noise that hides real problems.

# Design

- Make the census incremental: resume from where the last pass stopped (a rotating cursor), so every pane is probed within a bounded number of passes, and report coverage age instead of "budget reached".
- Size the per-pass budget from the measured probe cost, and skip panes whose agent record shows recent activity.
- Report only when a pane goes unprobed longer than a threshold.

# Acceptance Criteria

- A test with more panes than one pass's budget: every pane is probed within N passes, and no notice is emitted while coverage stays fresh.

# Notes

Filed by the coordinator after a night of repeated notices.
