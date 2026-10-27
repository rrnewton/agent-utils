---
title: 'agent-watch-recovery: classify blocked harnesses and retry transient stops'
status: open
priority: 2
issue_type: feature
created_at: 2026-10-27T16:01:30.595462304+00:00
updated_at: 2026-10-27T17:45:32.383977775+00:00
---

# Description

[GPT-6 Codex] Source 59543339 is published. Typed transient recovery, exact-pin stale-registration retirement, process anchoring, coordinator repair and the durable watcher are deployed. Only codex-lt, codex-std and codex-lead are opted into recovery; automatic turn interruption is disabled.

Remaining work: finish the session-follow hotfix and verify a healthy live report. The first live pass found unsupported launch-spec provenance "session-follow". The prior fake returned no command line, so it missed this branch. A real-command-line fixture fails before the correction; a second regression proves invalid capture metadata must not alter the queue binding. The correction refreshes through supported "anchor" provenance and validates before binding changes. All three session-follow tests now pass, including same-token/same-runtime preservation, interrupted binding-first recovery, real command-line capture and no queue change on invalid metadata. Full exact-head validation and hotfix publication remain pending. No live runtime was stopped or recovery input typed by the failure. Close only after the fixed binary is deployed and all three live session-follow rows are healthy.

The library classifies model capacity, API rate limits, unsafe commands, safety refusals, quota and stalled goals; only capacity/rate limits may retry with durable intent, bounded backoff and a three-attempt limit. Quota and safety/permission conditions notify only. Baseline failures are tracked as #586 host-baseline-contract; polling reply order as #587 chat-reply-order.
