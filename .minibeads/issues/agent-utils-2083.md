---
title: 'wrkslots-owner-retires-absent-row: a slot's live owner can retire its own absent-directory row'
status: open
priority: 2
issue_type: bug
labels:
- wrkslots
created_at: 2026-11-21T18:35:00-05:00
updated_at: 2026-11-21T18:35:00-05:00
---

# Description

When a slot's directory is gone, its row stays in the registry, and its live owner cannot retire it; only other paths can, so consumer launchers strand rows after a failed start.

# Design

Let the slot's verified live owner retire its own row when the slot directory is absent, with the same audit record as other retirements.

# Acceptance Criteria

- The live owner retires its absent-directory row; others still cannot; tested.

# Notes

Filed by the coordinator from a consumer report; the owning lane refines this file as it works.
