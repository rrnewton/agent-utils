---
title: 'agentctl-watch-hooks-trust-non-agent: watch treats a non-agent registry subdirectory as an agent'
status: closed
priority: 2
issue_type: bug
labels:
- agentctl
created_at: 2026-11-23T17:10:19-05:00
updated_at: 2026-11-23T17:10:19-05:00
closed_at: 2026-11-23T17:10:19-05:00
---

# Description

agentctl watch's hooks-trust sweep looped over a registry's non-agent
subdirectory (a hooks directory holding only hook scripts) and failed every
pass trying to load its missing agent record.

# Notes

## Resolution

[watermelon-full-preview] Resolved by 183dbb46 on main, the #2121 fix:
registry::names() lists only directories holding an agent.json in either
layout, so a non-agent subdirectory is never swept, and
answer_hooks_trust_if_dialog reads a missing record as no hooks to trust.
Pinned by a_missing_hooks_record_is_silent_in_either_layout, which covers a
hooks directory holding only a permission script in both layouts.
