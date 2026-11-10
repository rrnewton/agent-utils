---
title: "clean-caches-single-walk: clean-caches walks /proc once per command instead of once per cache"
status: open
priority: 3
issue_type: feature
created_at: 2026-11-10T10:30:00.000000000+00:00
updated_at: 2026-11-10T10:30:00.000000000+00:00
---

# Description

`wrkslots clean-caches` checks whether a gate is running in each slot (`_gate_running_in_slot`) with a full /proc
walk per cache, so its cost grows with slots times processes. Found while surveying for #1175 census-single-pass,
which left clean-caches out of scope.

# Design

Walk /proc once per command, index the processes by the slot paths they reference (cwd, and the gate admission
records if those are the source of truth), and answer each slot's question with a lookup. Keep the current skip
reasons and the --force-running behaviour.

# Acceptance Criteria

- A walk-count test: one /proc walk per clean-caches run, whatever the number of slots.
- Identical skip and remove decisions to today on a fixture with running and queued gates in some slots.

# Notes

Filed for the lead coordinator (2026-11-10), split out of #1175.
