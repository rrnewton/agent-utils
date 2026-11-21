---
title: 'vibe-talk-documents-backfilter-head-total: the Back walk reads the head before the grown-set refresh lands'
status: closed
priority: 0
issue_type: bug
labels:
- vibe-talk
- documents
- browser
created_at: 2026-11-20T18:00:00-05:00
updated_at: 2026-11-20T22:00:00-05:00
---

# Description

[watermelon-full-preview] In `vibe-talk/tests/documents_backfilter_browser.py`, after Back the walk waited for the list view with a card and for the starred card count, then immediately demanded the head read "Oldest first · 3 of 120 documents · newest at the bottom". Both waits are satisfied by the restored list as first drawn over the pre-growth total (100); the Back refresh that re-reads the grown set lands later under load, so the one-shot read saw "3 of 100" and failed. The cards themselves were correct. Verdict reported before fixing: a test race, not a page bug.

# Design

- After Back, condition-wait for the head line itself to show the grown total (`3 of 120`) before reading `list_facts`; keep the head, names and place assertions strict and unchanged.

# Acceptance Criteria

- `documents-backfilter-browser` passes repeatedly locally and in gate on a loaded host.
- A restored list whose head never reaches the grown total still fails, by the condition timing out.

# Notes

[watermelon-full-preview] mu-discussfix, stacked after #2020/#2021. Number 2023 pre-assigned by coordb.

## Resolution

[watermelon-full-preview] Landed on github/main as fd9feb35: clean full gate pass on 860f962d (replayed as fd9feb35).
