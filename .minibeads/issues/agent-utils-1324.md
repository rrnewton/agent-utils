---
title: 'per-kind-summarizer-model: choose the summarizer model per summary kind in vibe-talk and wrkviz'
status: closed
priority: 1
issue_type: feature
created_at: 2026-11-11T19:00:00.000000000+00:00
updated_at: 2026-11-11T20:30:00.000000000+00:00
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

## Resolution

[watermelon-full-preview] Done and deployed. vibe-talk per-kind models landed
as agent-utils `1c038e56` and wrkviz per-kind defaults/overrides as
`686d4428`. #1324 has been deployed since 12:12 PM ET (2026-11-11):
the coordinator applied the overlay (`message` and `thread_name` on
`small-elk-internal`, default `muse-spark-1.4-internal`), confirmed live in
app.log with answer_ms ~2 s. `side_agent` was switched to
`small-elk-internal` at about 2:17 PM ET on the owner's speed call (after the
A/B report recommended staying on Spark 1.4 for that kind; the owner chose
speed). Rollback remains deleting the override lines. The full bake-off is in
the harness report `ai_docs/reports/measurements/2026-11-11-small-elk-summarizer-ab.md`.
