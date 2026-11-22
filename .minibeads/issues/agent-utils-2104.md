---
title: 'wrkslots-refusal-class-json: type each recover-absent-validate-rows refusal with refusal_class and retryable'
status: open
priority: 1
issue_type: feature
labels:
- wrkslots
created_at: 2026-11-22T08:45:00-05:00
updated_at: 2026-11-22T08:45:00-05:00
---

# Description

A consumer's validation reaper retires absent validation rows with recover-absent-validate-rows --apply --format json and retries refused rows one by one. To decide which refused rows to ask again it must classify each refusal, but the JSON carries almost no typed signal: only the journal-busy status and the lock-busy reason; the seal-in-progress refusal is documented as retryable yet reported only as text, so the reaper matches a message prefix that breaks silently if reworded. On one host the worker log recorded about 16 distinct refusal kinds over a night, from transient census bounds to row-permanent conditions.

# Design

Add refusal_class and retryable to every refused rows[] outcome, and to the top-level object when a refusal aborts the whole command. Classes: busy (a live mutation holds what the call needs; retry soon), moment (host state at this moment; retry later), owner (clears when the recorded owner exits or its heartbeat expires), operator (an operator step is needed: unhold, recover, inspect), row (decided by the row record, this host identity and the wrkslots code; no retry helps until one changes). retryable is false only for row. Map every refusal raised on this command path to exactly one class at the raise site (typed exceptions where they exist), with a test that fails if a refusal on the path reaches the JSON unclassed. Python and Rust editions agree. This is command output only, no persisted state. Related: #2088 (judge row-only refusals before taking the locks) and #2100 (honour the recorded host alias in the locality check).

# Acceptance Criteria

- Every refusal on the command path carries a class; tests cover one refusal per class plus a whole-command abort; the seal-in-progress refusal is busy; the JSON schema and the user guide document both fields; Python and Rust agree.

# Notes

Drafted by a consumer's test lane from its reaper logs; filed by the coordinator.
