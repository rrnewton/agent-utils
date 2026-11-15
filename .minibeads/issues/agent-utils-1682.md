---
title: 'slack-rate-limit-budget: account for every Slack consumer and cut what empties the shared bucket'
status: open
priority: 2
issue_type: bug
external_ref: gh-1682
created_at: 2026-11-15T19:53:00.000000000+00:00
updated_at: 2026-11-15T19:53:00.000000000+00:00
---

# Description

The live page showed "Refresh failed · showing messages from 12:54" while the
server log reported Slack rate limits ("this route's bucket") on
`conversations.history` and `conversations.replies` from noon ET. Accounting
over the symptom window (12:00–2:00 PM ET, Nov 15) found every consumer of the
owner's Slack identity on the host sending ≈13.5 history calls/min and
≈3–5.5 replies calls/min in total — well under Slack's Tier 3 floor of about
fifty a minute per method — yet limiting on the upstream proxy's shared
per-route bucket. The drivers inside the page server are the page's background
poll plus the live poller (together ≈8.4 history/min sustained), Flat reads
fanning out to as many as twenty thread reads in one minute, a 600 s relist
burst, and a chat bridge's 15 s relay poll duplicating reads of channels the
page server also reads. Thread naming is not a driver since its cache-first
change and its channel pause.

# Design

Three cuts, in the order the coordinator approved:

1. Flat fan-out coalescing (page server): a Flat read made for the page's
   background poll or thread pane reads at most three threads that cost a
   call (a read a person is waiting on keeps ten), so successive polls
   coalesce the fan-out — the threads one poll reads cost nothing on the
   next, and the ones it skipped are read then. Overlapping reads keep
   their upstream reads: the settle rules that stop an older copy taking
   back an edit depend on them.
2. Per-caller budget split (page server): the page's background poll and the
   thread pane (`poll`, `side`) become a `Page` priority capped at half of
   each method's per-minute ceiling, refused at once when that share is spent
   (the message cache answers them) and paused while the provider is
   limiting, reserving the rest of the ceiling for live work and for reads a
   person is waiting on.
3. Relay-poll slowdown with a thread-listing cap (chat bridge): the relay
   poll's effective interval is floored at thirty seconds (a faster setting
   is still accepted and clamped, and the clamp is logged), and one poll
   lists at most three threads, the rest staying due for the next poll.

Live-instance cadence changes (a longer live poll, a longer relist, slower
backfill and naming) stay configuration proposals the coordinator applies;
this issue changes code only.

# Acceptance Criteria

- A Flat read for the page's poll or thread pane reads at most three
  threads that cost a call, and the existing overlapping-read tests
  (an older copy never takes back an edit) still pass unchanged.
- With a method's ceiling at twenty, page-priority reads are refused once
  ten calls of that method were sent in the minute while live and
  interactive reads still go, and page reads pause while the provider is
  limiting.
- A relay poll configured faster than thirty seconds polls at thirty
  seconds, and one relay poll lists at most three due threads.
- Each change carries a focused test in the owning crate's existing test
  layout, and the selected validation for the touched areas passes.

# Notes

[watermelon-full-preview] Accounting and ranked cuts were reported to the
coordinator before coding and approved as cuts (1), (5) and (2) above.
The bridge change is harness-neutral (Slack transport only), so no cell of
the harness support matrix changes.
