---
title: 'workctl-v0: a small work-item tool that tracks state, holder, stall timer and owner ETA'
status: open
priority: 2
issue_type: feature
labels:
- coordinator
created_at: 2026-11-22T10:20:00-05:00
updated_at: 2026-11-22T10:20:00-05:00
---

# Description

Work items move through planned, dispatched, implemented, review, approved, gated, landed, deployed and verified, but nothing records the state, so stalls are invisible.

# Design

A small CLI (journal per item in its own directory): new, bind, eta, advance (fact-checked for gated, landed and deployed), due (tick-hub ACTION lines) and list (the roll call). Tests drive the real CLI end to end. Used first for owner fast-track items only.

# Acceptance Criteria

- End-to-end CLI tests for each command and each fact-checked transition; a one-week trial on fast-track items is reported.

# Notes

Follow-up from the #2105 follow-through report; filed by the coordinator.
