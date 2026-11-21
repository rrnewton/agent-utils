---
title: 'wrkslots-eden-shared-bookmark-open: a shared bookmark makes any Eden draft count as open'
status: open
priority: 2
issue_type: bug
labels:
- wrkslots
created_at: 2026-11-21T13:55:00-05:00
updated_at: 2026-11-21T13:55:00-05:00
---

# Description

In a shared Sapling backing store, local bookmarks are shared across checkouts, and finish accounting counts any draft with a local bookmark as open (not lost), so a bookmark set in another checkout, or a plain local bookmark, lets a draft pass without being pushed.

# Design

Count a draft as open only on evidence it is published (remote bookmark or review), not on a local bookmark alone, without refusing ordinary drafts under review.

# Acceptance Criteria

- A local-only bookmark no longer makes a draft pass; drafts under review still pass; deterministic tests.

# Notes

Filed by the coordinator so the issue link resolves on main; the owning lane refines this file as it works.
