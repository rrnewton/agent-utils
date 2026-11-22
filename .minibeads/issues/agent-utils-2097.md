---
title: 'wrkviz-audit-coverage-muse-claude: audit-losslessness covers Codex only; add rule tables for Muse and Claude Code logs'
status: open
priority: 2
issue_type: bug
labels:
- wrkviz
created_at: 2026-11-22T06:40:00-05:00
updated_at: 2026-11-22T06:40:00-05:00
---

# Description

audit-losslessness reports Muse and Claude Code logs as 'not covered: no rule table exists', so no deletion of those originals can be gated on it. The sample found the archive keeps prompts, replies and tool calls and results verbatim, but drops Claude thinking and attachments and Muse runtime bookkeeping, unbound observer subagent logs and side files.

# Design

Rule tables for both formats, classifying each record type as retained, derivable or declared absent with a reason, so the audit can answer sound and lossless for them as it does for Codex.

# Acceptance Criteria

- The audit reports sound for synthetic Muse and Claude fixtures covering every record type the readers know; unknown types make it unsound; tests.

# Notes

Found by the #2095 archive-density sample; filed by the coordinator.
