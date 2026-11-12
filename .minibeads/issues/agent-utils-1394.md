---
title: 'discuss-glossary-preamble: two-layer warm-up design for Discuss'
status: open
priority: 0
issue_type: feature
assignee: mu-discussgloss
labels:
- vibe-talk
created_at: 2026-11-12T19:31:00+00:00
updated_at: 2026-11-12T19:31:00+00:00
---

# Description

[watermelon-full-preview] Owner (Slack 013, 023): a deterministic agent list and a brief project glossary in the Discuss preamble; a weekly-regenerated "top terms" glossary from docs, commits, and issues, seeded by the logged unknown terms; and a general systems primer ahead of the project glossary, common terms only. Design only, for owner review; no build.


Write `ai_docs/plans/2026-11-12-discuss-glossary-preamble.md` covering the agent list, the weekly top-terms glossary, the two-layer size split, and prefix caching.

# Acceptance Criteria

- (a) Agent list from the agentctl registry (#1342 purpose lines): live versus recent, privacy, freshness.
- (b) Weekly top-terms glossary: deterministic candidate selection from the #1393 frequencies plus docs/commits/issues, model-written reviewed definitions, size budget, consumer-state location; primer content stays out of agent-utils.
- Both layers feed #1352 `catchup_warmup_files`, primer first; the note proposes turning the warm-up on with the starter glossary.
- (c) Prefix caching answered from the gateway wire shape, or stated as not cheaply checkable.

# Design

See the plan file. Layer 1 (stable systems primer, ~2,000 chars, consumer-held) precedes Layer 2 (weekly project glossary plus agents, ~4,000 chars) inside the existing 6,000-char warm-up budget.

# Notes

[watermelon-full-preview] #1394 discuss-glossary-preamble, mu-discussgloss. Stays open pending the owner's go.
