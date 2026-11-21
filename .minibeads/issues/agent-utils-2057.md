---
title: 'vibe-talk-slack-rate-limit-check: are the conversations.replies limits a new poll storm or the normal floor?'
status: closed
priority: 1
issue_type: task
labels:
- vibe-talk
- slack
- rate-limit
created_at: 2026-11-21T10:46:00-05:00
updated_at: 2026-11-21T11:16:00-05:00
closed_at: 2026-11-21T11:16:00-05:00
---

# Description

[watermelon-full-preview] Owner request 282 (2026-11-21). The deployed server's log shows repeated `Slack RATE LIMIT ... on conversations.replies: gave up` warnings on All reads (threads' replies left out of an All read), including at 15:25:30Z and 15:26:30Z on 2026-11-21 with 15 and then 45 replies skipped. Two deploys landed that morning: 10:05Z (the train carrying #1986 slice 2's thread facts on the messages page, `c98efdb8`, with test-only `01306303` on top) and 10:40Z (`96c60423`, a scroll-measurement change). Question: did one of them start a `conversations.replies` poll storm (for example from #1986 slice 2's thread facts), or is this the normal rate-limit floor? Read-only check first; fix only with the coordinator's go.

## Findings (read-only, from the deployed server's `app.log`, 2026-11-21)

Method: the server's own per-window Slack budget reports (`Slack calls since the last report`, `by_method`/`by_caller` for `conversations.replies`, normalised by each window's `window_s`), plus the All-read warnings themselves. Server restarts in the log confirm the deploys at 10:05:55Z and 10:40:24Z.

| Window (2026-11-21, UTC) | replies/min | refused/min | All-read warnings |
|---|---|---|---|
| 09:05-10:05 (before both deploys) | 3.50 | 0.25 | 0 |
| 10:40-11:40 (after both deploys) | 3.02 | 0.26 | 0 |
| 10:40-12:17 (after, before owner activity) | 3.28 | 0.19 | 1 |
| 00:00-10:05 (whole pre-deploy stretch) | 3.16 | - | - |
| 10:05-10:40 (between the deploys) | 2.98 | - | - |
| 12:17-13:10 (owner active) | 6.57 | 1.67 | 20 |
| 15:18-15:30 (the reported incident) | 14.65 | 1.14 | 9 |

- The background callers (`backfill`, `live-poll`, `relist`) run at the same ~3/min on both sides of both deploys, with the same refused rate. No new background caller appears at either deploy.
- A new caller, `refresh` (the owner-facing refresh of a page already answered from the server's copy), first appears at 12:20:23Z, over an hour and a half after the second deploy, and the `counts` caller at 12:17:20Z. Every burst window (peaks at the configured 20/min ceiling for the method, `timeline.rs`) is `refresh`-dominated and coincides with the owner reading. All 41 All-read warnings that day fall in the two owner-active stretches (20 in the 12:00Z hour, 19 in the 15:00Z hour, 2 in the 13:00Z hour, none in any quiet hour).
- #1986 slice 2 cannot be the source: it counts thread facts from one indexed cache read at serve time (`ops::attach_thread_facts`, no provider read) and deletes the client's `readAllForCounts`, the read of All behind Main it replaced. `96c60423` touches no Slack code at all.

## Verdict

[watermelon-full-preview] The normal rate-limit floor, not a new poll storm. The background crawl holds ~3/min against the deployment's configured 20/min ceiling for `conversations.replies` (Slack tier 3, about fifty a minute, per `slack/budget.rs`) on both sides of both deploys; an owner-driven All read then legitimately drains the bucket thread by thread, and the reads that arrive while Slack still wants tens of seconds more are the ones that give up and skip replies. No fix taken: any change (a lower All-read fan-out, a longer give-up budget, a cheaper replies path) waits for the coordinator's go.

# Acceptance Criteria

- The counts above are reported to the coordinator with the verdict. (Done in the lane's first report.)
- No code change under this issue without the coordinator's explicit go.

## Resolution

[watermelon-full-preview] Closed read-only, coordinator concurring (2026-11-21): the `conversations.replies` limits are the normal rate-limit floor. Background callers hold ~3/min on both sides of both deploys (09:05-10:05Z 3.50/min vs 10:40-11:40Z 3.02/min, refused 0.25/min both sides, no All-read warnings in quiet windows); every burst is an owner-facing `refresh` All read at the configured 20/min ceiling during owner activity (12:17-13:10Z 6.57/min with 20 warnings; the reported 15:18-15:30Z window 14.65/min with 9). #1986 slice 2 reads thread facts from cache and deleted a provider read; it is not a poll storm, and no fix is taken under this issue.
