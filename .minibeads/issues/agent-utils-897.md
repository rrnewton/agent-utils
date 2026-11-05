---
title: 'chat-latency-stamps: stamp model, effort, role and protocol on each chat request at delivery'
status: open
priority: 1
issue_type: feature
assignee: cl-start
created_at: 2026-11-05T16:00:42.710740980+00:00
updated_at: 2026-11-05T16:00:42.710740980+00:00
---

# Description

An owner request that came out of the 2026-11-05 chat latency study. Chat request records carry nothing that would let latency be compared by model, effort or role: across all 534 requests in the study, every latency-fold field was "unknown". The full plan is in the harness repository's `ai_docs/2026-11-05-chat-latency-by-model-effort.md` (plan section).

# Design

1. At delivery, stamp each chat request record with:
   - the target agent;
   - its launch profile, harness, model and reasoning effort;
   - its role (coordinator, liaison or worker);
   - its protocol (tool-using vs hands).

   Read these from the agent record and launch profile. When a field is genuinely unknown, record it as unknown, never guessed.
2. Split response time into queue/think time and tool time, where the harness exposes it (for example, transcript tool-call timestamps).
3. The bridge writes jam markers: periods when delivery was blocked, such as a pane jam, a held queue or a stranded paste. `chat latency --exclude-jams` reports without them.
4. A weekly stats export (by model, effort, role and protocol) and 90-day retention of the stamped records.

# Acceptance Criteria

- New requests carry all of the stamps above. A test covers a known profile and an unknown harness.
- `chat latency` can group by model, effort, role and protocol, and `--exclude-jams` removes the jam periods.
- The tool/think split is present for harnesses that expose it, and marked unavailable otherwise.
- The weekly export runs, and records older than 90 days are pruned.

# Notes

Requested by the lead on 2026-11-05 at 11:00 AM ET (P1, an owner request via the latency study). Assigned to cl-start after #896.
