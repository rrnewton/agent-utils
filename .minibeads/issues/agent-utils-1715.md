---
title: 'vibe-talk-refresh-refused-stale-page: an owner page refresh loses the Slack budget to backfill/relist and stands stale until the next read'
status: closed
priority: 1
issue_type: bug
labels:
- vibe-talk
- cache
created_at: 2026-11-16T13:30:00+00:00
updated_at: 2026-11-16T13:30:00+00:00
---

# Description

Owner report, 2026-11-16 8:25 AM ET (screenshot): the page showed
"Refresh failed · showing messages from 07:33" — a page 52 minutes stale.

Live log (`vibe_talk::slack::budget`, consumer `slack_calls`): no Slack rate
limiting (every report since 7 AM ET has `limited=0`), but this server's own
budget refused 1-3 reads in many windows
(`refused_by_method=conversations.history/replies`) while backfill/relist used
6-20 calls a minute. At 13:17:12 UTC: refused=1 with
`refused_by_method=conversations.history=1`, then
`WARN vibe_talk::ops "a page answered from the message cache could not be refreshed; it stands until the next read" consumer="timeline_fallback"`.

## Root cause (to verify in code)

- The upstream read behind a cache-first answer runs hurried
  (`hurry(true)`): at the method ceiling it is refused at once, while relist
  and the live poller (Live priority, patient) wait for room and take it.
  The refresh therefore starves under exactly the load that makes it needed.
- A failed refresh backs off 60 s doubling to 300 s in the refresh schedule,
  and the served page's refresh has no retry at all: it stands until the
  next owner read. The banner clears only when a later read lands fresh.

## Fix

- Owner-facing page refreshes must not lose to background backfill/relist in
  the budget (a priority above Live, plus a reserved share of each method's
  ceiling that Live work cannot take). Do not raise the overall Slack call rate.
- A refused refresh retries soon, driven by the schedule's due instant
  (condition based), instead of standing until the next read.
- The banner clears once a refresh succeeds (the landed/live read says fresh).

## Tests

- Unit level with a fake budget: with a method's ceiling filled by Live and
  Crawl work, a refresh is still admitted within its reserved share while
  Live waits and Crawl/Background are refused.
- A budget-refused refresh becomes due again in seconds, not minutes, and a
  success clears the back-off.
- Screenshot the banner states (stale, then cleared) on a test instance.

# Acceptance Criteria

- With backfill/relist saturating a method, an owner-facing refresh is
  admitted ahead of them without raising the overall call rate.
- A refused refresh retries on the server's own schedule within seconds and
  recovers without the owner reading again; the banner clears on success.
- Focused tests pass; the owner-tier gate passes with path selection.

## Resolution

[watermelon-full-preview] Landed on `github/main` as 6df5e81b (gate green on the
full run; gated tree e18b8f35, replay-base 9a5e0ff7). Owner-facing refreshes now
run at a new budget priority ahead of live/background work with the last quarter
of each method's ceiling reserved for them and wait for that budget's room
(never out a provider Retry-After); a refusal made by this server's own budget
without sending retries in its own task when the refresh schedule comes due
(5 s doubling to 60 s), and a success clears the schedule so the banner clears
on the landed/live read. Unit tests with a fake budget cover admission in the
reserved share, the seconds-scale refused-retry, and the never-sent
classification; the full vibe-talk suite and the four previously failing
cache-serve integration contracts pass. Banner states screenshot on a test
instance (fake API, phone and desktop): stale
"Refresh failed · showing messages from 04:54", then cleared
"Live · updated 05:46". Deployment is coordb's step.
