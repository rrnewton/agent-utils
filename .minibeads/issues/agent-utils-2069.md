---
title: 'vibe-talk-discuss-glossary-per-channel: summaries need a project glossary per channel'
status: open
priority: 1
issue_type: bug
labels:
- vibe-talk
- owner-ui
created_at: 2026-11-21T13:55:00-05:00
updated_at: 2026-11-21T13:55:00-05:00
---

# Description

The owner: "What the hell have we not provided any glossary at all to our summary agent. This is crazy that it would not know about detcore. That would be in any possible project glossary that we could come up with". The summariser's warm-up glossary covers only one project's terms, for every channel.

# Design

Per-channel glossary selection (channels or name patterns mapped to glossary files, plus a default list), back-compatible with the single warm-up list; a daily harvest of the unknown-terms log into a reviewed proposal of additions, never auto-applied.

# Acceptance Criteria

- A channel loads its project's glossary.
- The harvest writes a proposal file; tests are deterministic.

# Notes

Filed by the coordinator so the issue link resolves on main; the owning lane refines this file as it works.
