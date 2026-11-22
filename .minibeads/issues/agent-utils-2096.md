---
title: 'wrkviz-codex-audit-unaccounted-shapes: the Codex losslessness audit is unsound on current rollouts'
status: open
priority: 2
issue_type: bug
labels:
- wrkviz
created_at: 2026-11-22T06:40:00-05:00
updated_at: 2026-11-22T06:40:00-05:00
---

# Description

On a median current Codex rollout lineage, audit-losslessness exits 1 with 'sound: NO': 134 rows have no rule (response_item additional_tools, configuration_update, token_usage_record) and 5 duplicate claims are unverified. Developer instruction text is declared absent and appears nowhere else; command rows keep text but lose exit code, duration, cwd and the stdout/stderr split.

# Design

Add rules for every current row shape so the audit is sound (every row accounted for), verify the duplicate claims, and decide per shape whether it is retained, derivable, or declared absent with a reason. Keep developer instructions and command metadata if they are cheap.

# Acceptance Criteria

- audit-losslessness reports sound on current rollout fixtures (synthetic, no real transcript content); a new unknown shape makes it unsound, with a test.

# Notes

Found by the #2095 archive-density sample; filed by the coordinator.
