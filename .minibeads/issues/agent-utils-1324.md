---
title: 'per-kind-summarizer-model: choose the summarizer model per summary kind in vibe-talk and wrkviz'
status: open
priority: 1
issue_type: feature
created_at: 2026-11-11T19:00:00.000000000+00:00
updated_at: 2026-11-11T19:00:00.000000000+00:00
---

# Description

The owner (2026-11-11, replies 371/372) ordered a switch of vibe-talk message
summaries and thread names to a newer small internal Muse model (preview),
`small-elk-internal`, ahead of the finished bake-off, and widened the mechanism:
the summarizer model becomes a per-kind choice for every summary kind, so a
later bake-off can assign a model per task with no code change.

Today one `[summaries.llm]` block (one model) serves every vibe-talk kind, and
wrkviz has one phase default plus one rollup default.

# Design

- vibe-talk: optional `[summaries.llm.models]` table with keys `message`,
  `thread_name`, `digest` and `side_agent`, each falling back to
  `summaries.llm.model` when absent. Cache keys use the EFFECTIVE model per
  kind. First landing.
- wrkviz: a per-kind default (phrase/paragraph phases, daily rollup) plus a CLI
  override, Python and Rust. Second landing.
- The switch itself is configuration only: the coordinator applies overlay
  lines `message` and `thread_name` to the preview model; `digest` and
  `side_agent` stay on Spark 1.4. Rollback is deleting the override lines.

# Acceptance Criteria

- Config tests cover parsing, fallback and refusal of unknown kinds/models.
- A meta_gateway test proves each kind sends its own effective model.
- Cache/policy keys differ when only a per-kind override changes.
- The vibe-talk landing passes the owner-priority gate; wrkviz follows.

# Notes

Related: #1275 muse-ember-summarizer (the bake-off this mechanism serves),
#1027 vibe-talk-spark-summarizer (gateway backend).
