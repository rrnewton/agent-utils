---
title: 'vibe-talk-scroll-stability-arrival-offer: the jump offer is checked after a fixed quiet, not waited for'
status: open
priority: 0
issue_type: bug
labels:
- vibe-talk
- scroll
- browser
created_at: 2026-11-20T18:00:00-05:00
updated_at: 2026-11-20T18:00:00-05:00
---

# Description

[watermelon-full-preview] In `vibe-talk/tests/scroll_stability_browser.py`'s Main walk, after a live message arrives, the walk waited for the timeline request count to rise, took a fixed `reader.quiet()` (400ms + 3 frames), judged the reader's row unmoved, then immediately checked `#jump-newest` for `data-arrivals` and failed in gate with "nothing offered the reader up the list the message that arrived" (line 745). The offer is painted by a later batch than the row/request that triggers the wait, so on a hot host the fixed quiet elapses first. Verdict reported before fixing: a test problem, not a page bug — the same file's `Reader.settled()` already documents that a fixed quiet assumes the page finished within the window and under load it has not.

# Design

- Condition-wait for the offer itself (`reader.wait_for` on `#jump-newest[data-arrivals]`) after the arrival, keeping `still()` as the position claim. No fixed `quiet()` as the only wait before the check.

# Acceptance Criteria

- `scroll-stability-browser` passes repeatedly locally and in gate on a loaded host.
- A live arrival that genuinely offers no jump still fails, by the condition timing out.

# Notes

[watermelon-full-preview] mu-discussfix. Number 2021 pre-assigned by coordb. Sibling: #2020 vibe-talk-read-pause-reply-hold-clock.
