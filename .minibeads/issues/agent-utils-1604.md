---
title: 'start-bind-retry-loud: retry a transient slot-owner bind refusal and keep its failure visible'
status: open
priority: 2
issue_type: bug
external_ref: gh-299
created_at: 2026-11-15T17:47:04.000000000+00:00
updated_at: 2026-11-15T17:47:04.000000000+00:00
---

# Description

A slot-owner bind refused for a transient non-lock reason is not retried, agentctl start still exits 0, and the refusal is not visible to a later status, doctor, or health check, so slots accumulate with no owner generation recorded. The busy-lock case is already handled (#786 agentctl-start-slot-bind-busy-lock, closed: bounded busy-lock retry, a loud warning naming agentctl rebind-slot, and the rebind command). On current main both retry loops stop unless the error says the lock is busy, so the reported transient refusal (which succeeded seconds later by hand) is still dropped after one attempt, and the refused outcome appears only in the start result, never persisted.

# Design

Retry the bind a bounded number of times with backoff for transient refusals, not only a busy lock. If it still fails, exit non-zero or persist a bind-owed state that status/doctor surfaces with the exact agentctl rebind-slot NAME remedy.

# Acceptance Criteria

- A fake slot manager refusing once transiently then succeeding ends bound with no operator step.
- A permanently refusing one makes start fail loudly (or records an owed bind that doctor lists).
- A busy-lock refusal behaves as today.

# Notes

[watermelon-full-preview] Filed from GitHub #299 start-bind-retry-loud (https://github.com/rrnewton/agent-utils/issues/299) during Request 098 triage. Extends #786 (busy lock only); do not reopen its lock scope.
