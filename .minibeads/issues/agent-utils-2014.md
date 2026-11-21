---
title: 'vibe-talk-screenshots-desks-scene-59-scroll: scene 59 strict scroll equality races late re-anchoring on a hot host'
status: closed
priority: 0
issue_type: bug
labels:
- vibe-talk
- ui
- browser
created_at: 2026-11-20T18:00:00-05:00
updated_at: 2026-11-20T22:00:00-05:00
---

# Description

[watermelon-full-preview] `vibe-talk.shots.screenshots-desks` has been red in most gates since browsers returned, at scene `59-all-thread-selected`: "state was NOT reached: still All, the thread selected, and nothing scrolled", sometimes with `TargetClosedError` noise. Solo it is green. Report before fixing found no page bug: selecting by the strip (`threadstrip.ts`) only sets `data-focus-thread` / `data-thread-in` / `aria-pressed` and a status line and writes no scroll. The act waited for scroll stability around the click but never for stability at the recorded offset, while `capture()` demands strict `scrollTop === __beforeTop` for 5 unstretched seconds, so any late re-anchor settling at a different offset makes the claim unrecoverable. The failing scroll delta was not captured despite instrumented runs (about 10 of 11 desk runs passed locally), and that gap was reported rather than filled in.

# Design

- In `_act_all_thread_selected`, after the click and the focus wait, wait until `#scroll-area.scrollTop === __beforeTop` and stable (same 3-frame + 300ms condition as `_wait_scroll_stable`, patience-stretched by the page waits) via a new `_wait_scroll_stable_at` helper.
- On timeout, read the live offset back and fail with the delta (`selecting the thread scrolled #scroll-area from X to Y (delta Z)`), so a real scroll on select is reported as page-bug evidence instead of a stale-equality timeout.
- Keep `capture()`'s strict equality as the assertion. No page change.
- Separately, if cheap, close the page before its context in the screenshot runner so the post-ok `TargetClosedError` teardown noise (seen solo after an `ok` line, exit 0) does not leak into gate logs.

# Acceptance Criteria

- Scene `59-all-thread-selected` passes in the full desk matrix repeatedly on a loaded host (gate context), with `capture()`'s strict scroll equality unchanged.
- A genuine scroll on select, if one exists, fails with its measured delta rather than "state was NOT reached".
- The desk screenshot step no longer prints post-ok `TargetClosedError` teardown noise (or that part is reported as not cheap and split out).

# Notes

[watermelon-full-preview] mu-discussfix, after #2012. Number 2014 pre-assigned by coordb. Evidence: #2010 gate (head b908f455) screenshots-desks exit 31 at scene 59 after scene 58 passed on laptop-1280; solo `--only 59-all-thread-selected` 4/4 ok; full desks about 10/11 pass locally including parallel dagrun sets. Validation deferred during the coordinator's measurement window on the lane host; code written first, browser proof to follow when the window lifts.

## Resolution

[watermelon-full-preview] Landed on github/main as 9b3384de: screenshots-desks green in the gate on 048304d0 (stack replayed to 9b3384de); desks matrix 3/3 locally.
