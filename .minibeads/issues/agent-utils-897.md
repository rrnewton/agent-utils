---
title: 'chat-latency-stamps: stamp model, effort, role and protocol on each chat request at delivery'
status: closed
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

## Resolution

[opus 5.5] cl-start, 2026-11-05.

The lead's three refinements are in.

- **Stamps at delivery.** Each request record gets a `stamp` when it is delivered: target agent, launch profile, harness, model, effort, adapter, role and protocol. These come from the agent record, its profile in the registry's `profiles.json`, and the coordinator designation.
  - Role is `coordinator` for the designated coordinator. Otherwise it is the profile's new `role` key (`coordinator`, `liaison`, `coordinator-hands` or `worker`), and `worker` if that is unset. It is never read from the agent's name.
  - Protocol is the profile's new `protocol` key (`self-tooling` or `hands-delegating`), or `unknown`.
  - Both editions check the two profile keys. Any other field that no source gives is `unknown`.
- **Think/tool split.** The transcript reader now shares prompt and tool-call times (`TurnMarks`). At the first reply, the request records `turn_timing`: the prompt's time and the first tool call after it, with split `transcript`. Without a followed transcript the split is `unavailable`.
  - Day summaries gain the stages `delivered_to_first_tool` and `first_tool_to_first_reply`.
  - The listing shows them as thinking and tools.
- **Grouping.** Day summaries are keyed by every stamp plus a jammed flag. A summary whose stamps are all unknown keeps its old directory name; any other adds a short digest. `chat latency --by` takes any comma-separated mix of `profile`, `model`, `effort`, `adapter`, `harness`, `launch-profile`, `role`, `protocol` and `coordinator`.
- **Jams.** A jam is recorded only for a blocked delivery, never for an agent that was busy.
  - Blocked means the errors named a stranded paste or draft, the pane's lock, a held or paused queue, a retried refusal, or a target that is down.
  - The span runs from the first such error to the last, or to delivery. Spans of 5 minutes or more go to `latency-jams.jsonl`, with their cause.
  - `chat jam` records one by hand.
  - `--exclude-jams` drops requests admitted inside a jam: from the listing at once, and from `--by` for requests folded after the jam was recorded.
- **Export and retention.**
  - Each week the service writes `latency-exports/<ISO week>.json` for the last full week. `chat latency --export` writes it on demand.
  - Every folded request also leaves a stamped row, without its text, in `latency-records/<ET day>.jsonl`.
  - After 90 days the service prunes day summaries, stamped rows, exports and jams. Request records are never pruned by it.

**Tests:**
- Stamps from a known profile (and as designated coordinator), and from an unknown-harness record.
- The split from transcript marks, and unavailable without them.
- A blocked delivery marks a jam; a busy one does not.
- A 91-day-old latency row is pruned while a live (backdated) request record stays.
- `--exclude-jams` on the listing.
- Grouping, jam checks and causes, the weekly export and pruning in `chat_latency`.
- Profile role and protocol checks in both editions.

**Not covered:**
- A bridge outage is not detected on its own; an operator records it with `chat jam --cause bridge-restart`.
- Day summaries folded before a hand-recorded jam keep their jammed flag. The listing and the stamped rows can still be filtered against later jams.
