---
title: 'discuss-summarize-button: Discuss asks only when told, over the last N unread since a time'
status: open
priority: 0
issue_type: feature
assignee: mu-catchup
labels:
- vibe-talk
created_at: 2026-11-12T13:20:12+00:00
updated_at: 2026-11-12T13:20:12+00:00
---

# Description

[watermelon-full-preview] From the owner's first-impressions list (reply 413, relayed by coordb): opening Discuss must not run anything. Slice 1 prefetch-on-open spent a model call (28-42 s live) before the owner asked, and the page let a second press start a second request while one was in flight (live 2026-11-11).

Parent: #1316 catchup-side-agent. Cache behaviour: #1348 discuss-cache-only (landed).

# Design

- Opening the panel shows the saved state only (transcript lands in #1345) and asks nothing of the model. Beside Close sits **Summarize last [N] unread since [HH:MM]**: N and the time are the two ends of one choice over the scope's unread set (from the cache, no model call) — editing N moves the time to the Nth-newest unread message, editing the time moves N to the unread at/after it.
- Pressing it runs the one catch-up over that window (`since` bound applied server-side to the cache window; labels/footer as slice 1). While a request is in flight, every Discuss entry point (bar button, menu item, the Summarize button itself) is disabled and further presses do nothing — no second request, no close-reopen toggle.
- The unread preview comes from the same cache-only fetch (ids + times, no model): server endpoint + contract field, so phone and desktop agree on N/time.

# Acceptance Criteria

- Open (bar or menu, phone or desktop) fires no catch-up POST; the Summarize control names the current N and HH:MM for the scope.
- N and time stay linked both ways over the real unread set; Summarize posts exactly that bound and the answer/footer cover exactly its unread.
- While a request is pending, all Discuss entry points draw disabled and extra presses send nothing; they re-enable on answer or error. Page tests pin the linking and the guard; browser walks cover phone 390x844 + desktop; gated at owner tier, landed on its own.

# Notes

Owner replies 413/414. Number reserved by coordb. Sibling layout work (#1344/#1354, mu-discusslay) owns the panel header shell; the Summarize button goes in its actions slot next to Close.
