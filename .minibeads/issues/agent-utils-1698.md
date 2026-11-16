---
title: 'agent-cli-slack-reads-through-vibe-talk: point agent-facing read recipes at the server'
status: open
priority: 2
issue_type: task
labels:
- slack
- vibe-talk
created_at: 2026-11-16T02:00:00.000000000+00:00
updated_at: 2026-11-16T02:00:00.000000000+00:00
---

# Description

Agent-facing text still teaches direct Slack reads: the embedded chat userguide and quickstart, the chat skill, and crate examples describe running user describe, member listing, message listing and file download commands against Slack directly, and reading a thread back with a send-plus-list round trip. Each such run is a direct history, replies, users or file spend with no cache and no shared budget, at an unmeasured ad-hoc rate. The compliant readers already exist: the vibe-talk CLI and tool subcommands against the running server, and the bridge-state chat readers.

# Design

Rewrite every agent-facing read recipe to use a vibe-talk-backed reader (the server API or the vibe-talk CLI subcommands) or the local bridge-state readers, and remove the direct Slack read recipes from the userguide, quickstart, skills and examples. Write recipes stay only where the single-writer work keeps them, and say so.

# Acceptance Criteria

- No agent-facing userguide, quickstart, skill or example instructs a direct Slack history, thread, user, member or file read.
- Each removed recipe is replaced by a working server-backed equivalent with the same information, or is named as removed with its reason.
- The documented-defaults and embedded-guide checks pass.

# Notes

[watermelon-full-preview] Filed by the direct-Slack burn-down enumeration (owner request 105, 2026-11-15): the target model is client <> vibe-talk <> Slack, and this is one remaining path that reaches Slack without vibe-talk's cache or writer in between. Rates and the shared-budget accounting are in #1682 slack-rate-limit-budget.
