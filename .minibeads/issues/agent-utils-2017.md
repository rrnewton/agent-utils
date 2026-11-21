---
title: 'wrkslots-hooks-per-slot-type: post-provision hooks have no per-slot-type selection or skip condition'
status: open
priority: 2
issue_type: bug
labels:
- wrkslots
created_at: 2026-11-21T11:45:00-05:00
updated_at: 2026-11-21T11:45:00-05:00
---

# Description

A consumer's post-provision hook (a dependency-regeneration step) now runs for agent slots too and fails on a proxy refusal, so create refuses. Recover also requires the coordinator pid in ancestry, so a lane cannot abort its own half-made create.

# Design

Let hooks declare which slot types they apply to (or a skip condition), and let a lane recover or abort its own half-made create without the coordinator's pid.

# Acceptance Criteria

- The behaviour described under Design, covered by a deterministic regression test (condition-based, no wall-clock budgets).

# Notes

Filed by the coordinator so the issue link resolves on main; the owning lane refines this file as it works.
