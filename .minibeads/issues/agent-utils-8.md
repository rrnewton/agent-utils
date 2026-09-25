---
title: 'gchat-thread-listing: return the expected linked conversations'
status: open
priority: 0
issue_type: task
labels:
- vibe-talk
- threads
- correctness
depends_on:
  agent-utils-1: parent-child
created_at: 2026-09-25T01:32:03.040463709+00:00
updated_at: 2026-09-25T01:33:45.753030675+00:00
---

# Description

[gpt-5.6-sol] Diagnose why the thread menu can show a thread unrelated to the expected linked conversations. Keep the fix at the provider-neutral boundary where possible and private adapter behavior outside OSS.

# Acceptance Criteria

Given multiple linked conversations, the API and UI show the expected thread identities in deterministic order with no unrelated entry; tests reproduce the original mismatch.

# Notes

Reported symptom: the Threads menu exposes one unexpected entry instead of either expected linked conversation. Trace provider thread discovery, adapter normalization, and UI filtering before changing presentation.
