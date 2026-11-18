---
title: 'vibe-talk-discuss-default-bound-since-last-summary: the default Summarize bound starts where the last summary in this scope ended'
status: closed
priority: 1
issue_type: task
labels:
- vibe-talk
created_at: 2026-11-18T09:32:42-05:00
updated_at: 2026-11-18T09:32:42-05:00
---

# Description

[watermelon-full-preview] Owner (bridge 174, 177): "When I do a summary what is the DEFAULT it is setting for how many hours to go back? I think the default should be the time since the LAST SUMMARY. So if I use it N times in a row I get a contiguous coverage of time split up into those chunks." And: "When in thread view I think the default TIME should be all the way back to start of thread (all unread) unless there has been a prior summary in which case it should go back to the moment of the prior summary."

Today the panel sets the bound to all unread on preview load, and the hours field just restates the age of the oldest unread (hence values like 9716 hours), with no last-summary awareness. Builds on #1841's per-scope Discuss state.

# Design

- Per scope (the channel's main scope, and each thread), the default start of the bound is the moment the previous summary in this scope ended — its covered-until time — so repeated Summarize presses give contiguous, non-overlapping coverage. The count and hours fields reflect that start; the reader can still edit them.
- With no prior summary in the scope: main defaults to all unread; a thread defaults to the thread's start (all its unread).
- The covered ranges are kept per scope (not just the last end): every successful summary in a scope records the range it covered, merged where ranges overlap, persisted on the device with #1841's per-scope state so the default survives a refresh. A scope's saved server record also seeds the last end where no device record exists.
- The follow-up uncovered-messages line (#1843) reads these same per-scope covered ranges.

# Acceptance Criteria

- Two summaries in a row in a thread and on main: the second's default bound starts exactly where the first ended.
- First-ever summary in a thread starts at the thread start (all unread).
- The count and hours fields show the default start and remain editable.
- The per-scope covered ranges persist across a refresh.
- Page tests at phone and desktop sizes; screenshot matrix (two summaries in a row, bounds visible; thread and main; phone and desk; light and dark) inspected.

# Notes

Lands after #1841 (per-scope Discuss state). The uncovered-messages line that reads these covered ranges is a separate issue.

## Resolution

[watermelon-full-preview] Fixed on this branch: the panel now keeps covered ranges per scope (`discussCoverage` device setting, keyed by channel plus thread-or-main; the saved server record seeds a scope's covered end where the device kept none). A successful Summarize records the range it covered (merged with the scope's held ranges), and a scope's preview now defaults its bound to start exactly at the scope's last covered end — count, since-time and hours all restate that start, and the fields stay editable. With no prior summary a thread defaults to its start (all unread) and main to all unread. Tests: two summaries in a row on thread and on main asserting the second's bound starts exactly where the first ended and the first-ever thread summary starts at the thread start, plus a reload-through-kept-coverage test (new tests fail on the unpatched base, pass patched); settings and catchup suites and the web typecheck pass. Screenshot matrix on a fake-API test instance: /tmp/mu-discussscope2-shots-1842/ (phone and desktop, light and dark, keyboard open and closed) shows the first bound, first summary, second bound starting at the first end, and second summary in main and in thread A, all inspected.
