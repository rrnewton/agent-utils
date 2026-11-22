---
title: 'gate-known-red-aware-verdict: gate rows mark steps that are on the known-red list'
status: open
priority: 2
issue_type: feature
labels:
- coordinator
created_at: 2026-11-22T10:20:00-05:00
updated_at: 2026-11-22T10:20:00-05:00
---

# Description

Lanes rerun steps that fail on unrelated trees and clean main, costing hours on fast-track work.

# Design

A known-red list (with tracking issues) read by gate.py; the ledger row records which red steps are known-red, and the summary says when ONLY known-red steps failed so the coordinator can waive at once.

# Acceptance Criteria

- Tests: a row with only known-red failures is labelled so; an unknown red is never labelled known-red.

# Notes

Follow-up from the #2105 follow-through report; filed by the coordinator.
