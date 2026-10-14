---
title: 'dagrun-wall-timeout-multiplier: scale wall-clock step ceilings per machine, separately from CPU scaling'
status: open
priority: 4
issue_type: feature
labels:
- dagrun
created_at: 2026-10-14T14:48:59.073649108+00:00
updated_at: 2026-10-14T14:48:59.073649108+00:00
---

# Description

[opus 5.5] Found by the 2026-10-14 branch triage (ai_docs/transient/2026-10-14-branch-triage/REPORT.md). archived/rescue/20260918-b7366e40-independent-dagrun-wall-timeout-multiplier adds --wall-timeout-multiplier, DAGRUN_WALL_TIMEOUT_MULTIPLIER and DAGRUN_WALL_TIMEOUT_PLATFORM in both editions. It scales the resolved wall budget after the existing four rules, with config carry, journaling, refusal of the new keys in DAG documents, docs and tests. On main only the CPU-derived backstop follows --cpu-timeout-multiplier; explicit step timeouts and default_step_timeout are never scaled per machine. No current user has asked for it, so it is low priority. Landing: rebase b7366e40 (the dagrun model, scheduler, CLI and io have moved since), update every resolved_wall_timeout call site in both editions, and extend the refused-keys lists in io.py, io.rs and cross/differential.py.

# Acceptance Criteria

Both editions scale wall ceilings by the new multiplier identically (the differential agrees), refuse the new keys in DAG documents, and document them.
