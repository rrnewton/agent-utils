---
title: 'vibe-talk-next-plan: one terminology and one master plan for storage, sync, summaries, research, voice and the Work tab'
status: open
priority: 1
issue_type: task
labels:
- vibe-talk
- design
created_at: 2026-10-23T14:19:52.136237406+00:00
updated_at: 2026-10-23T14:19:52.136237406+00:00
---

# Description

[opus 5.5] The owner, 2026-10-23, starting a big planning phase: keep evaluating the overall design and make the plan documents fit together: the server-side research agent exposed in the channel UI, the Rust wrkviz port and its backend, the voice agent, and the caching story (does one message store serve live serving, the wrkviz-style view and a clean sync model with fast local updates and server merges?). API/SDK over CLI for the research backend is settled. Settle the terminology (helper, research agent, side agent; deterministic summaries versus open-ended questions). Propose a version scheme: vibe-talk is still 0.0.1.

# Design

ai_docs/transient/2026-10-23-vibe-talk-next/: TERMINOLOGY.md (one page, recommended terms and rejected alternatives) and README.md (architecture, interfaces, the one data model, caches, sync, milestones mapped to versions, dependencies, in-flight lanes, risks, every open owner question deduplicated). The earlier plan documents get a header pointing to it, terms edited in place, superseded sections marked with a dated note.

# Acceptance Criteria

Docs landed on main through the hygiene gate; every earlier plan document points to the master plan and uses its terms; the consolidated question list covers every question in those documents.
