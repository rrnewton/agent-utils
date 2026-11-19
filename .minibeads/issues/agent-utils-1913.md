---
title: 'vibe-talk-search-typing-lag: typing in the search field stays instant while everything slower runs off the input path'
status: closed
priority: 0
issue_type: bug
labels:
- vibe-talk
- search
- ui
- performance
created_at: 2026-11-19T14:20:00-05:00
updated_at: 2026-11-19T17:30:00-05:00
closed_at: 2026-11-19T17:30:00-05:00
---

# Description

[watermelon-full-preview] Owner report (thread 224, 2026-11-19 2:17 PM ET, verbatim): "I am noticing serious lag while typing during search... hmm... the goal was to keep it fast/responsive and local while I type. Anything slow should be async and not slow my typing"

Goal: typing in the search field stays instant. Local matching runs per keystroke cheaply; everything slower (server hits, link and thread-name lookups, chip counts, anchor holds and re-marks, big re-renders) runs async or debounced and never blocks the input event.

Liaison additions from the #1899 review (2026-11-19): (A) Profile with the pinned bucket chips on: the owner's lag may come from them (5730d703, 41e5d9e5). (B) Correctness bug to check while in that code: after a new search 'old lantern' that shows '0 of 60 loaded' and 'None match', the pinned chips still read '1 in main / 2 in threads', the counts from the previous 'lantern' search. Chip counts must reflect the current query, but recompute them off the input path (idle/debounced) and cheaply, never per keystroke synchronously. If (B) is real, fix it in this change and show a before/after shot. Nit if cheap: in dark-phone-keyboard-chips-after-scroll-bottom the 'ON THIS SCREEN' heading peeks half-cut between the action row and the chip panel.

Suspects deployed today, to bisect by measurement: #1899 pinned bucket chips (5730d703, 41e5d9e5: chip counts recomputed per keystroke?); #1894/#1901 anchor hold and re-mark (253ff5ab, 747cd7a3, e9d687c3, de9931f9: layout reads on every input?); #1898 link-label unwrap (7b9f8dbb: re-parsing every message per keystroke?); #1895 link search; windowed lists (#1846).

Prior search-responsiveness work searched before filing: #1786 vibe-talk-search-scope-server-hits, #1797 vibe-talk-search-bucket-toggles, #1894 vibe-talk-search-extra-hits-off-by-default, #1895 vibe-talk-link-search-semantics, #1899 vibe-talk-search-bucket-chips-pinned, #1901 vibe-talk-search-late-hits-shift-anchor. No in-flight lane owns typing-path main-thread work; mu-linksearch/1895, mu-chippin/1899, mu-nestedlink/1898 are the suspect parents to bisect against.

# Design

- Measure first on a large fixture channel (thousands of messages, many links and threads) on a test instance (never live): the main-thread work per input event (Performance API long tasks, or counting synchronous render/match/layout calls per keystroke), on main versus each suspect's parent, with the pinned bucket chips on and off. Find the blocking work; do not guess.
- Fix it so the input handler itself is cheap: keep local matching synchronous but cheap (incremental, cached normalised text per row/message, folded once); move chip counts, server and link lookups, anchor re-marks and heavy re-renders (seams, tallies, link-kind counts, server scheduling, place-keeping layout reads) behind requestIdleCallback or a debounce, or into a worker; never force layout (getBoundingClientRect/scrollTop reads) per keystroke on the typing path.
- Chip counts (B): counts always reflect the current query. While a new query's answer is pending the stale counts are not shown (the row hides or reads as pending without the previous query's numbers); the recomputed counts land off the input path (idle/debounced, from the server answer) and cheaply, never recomputed synchronously per keystroke.
- Results still update correctly after typing: after the pause the full pass lands the exact rows, counts, empty sentence, seams and marks the synchronous pass lands today, at phone and desktop sizes. Condition waits only (owner rule 346: no wall-clock thresholds in a gating check; count synchronous work per input event — e.g. full re-renders or layout reads per keystroke must be O(1), not O(messages) — or long tasks with generous thresholds).
- Nit if cheap: the 'On this screen' separator never peeks half-cut under the pinned chip panel in the keyboard-open after-scroll-bottom scene.
- Page tests at phone and desktop sizes for the exact defect, in light and dark where theme-related and keyboard open where layout-related, with condition-based waits only. Dynamic testing on a real server through the real router (owner thread 185): exercise the owner's exact sequences on a test instance and attach evidence; a page test that injects a fake client call does not count. Screenshot matrix on a test instance (light and dark, keyboard open with a really shrunk viewport and closed, phone 412x915 and desktop 1280x800, Discuss half-pane open in at least the phone shots, plus the targeted before/after states), from a fresh build rebased onto current github/main with BUILD.txt in the harness format, using the shared screenshot harness.

# Acceptance Criteria

- Before/after numbers (measurement, not gating) on the big fixture: per-keystroke main-thread time and long tasks while typing, from the measurement, on phone and desktop profiles, with chips on; after the fix the input event does no O(messages) synchronous work (no full-list re-render, seam pass, thread/link scan or layout-read loop per keystroke).
- Typing a query that matches, refines, and then matches nothing: rows filter per keystroke from the cheap local pass, and after the pause the full result (counts, 'N of M loaded', empty sentence, hit marks) is exactly what the current build shows for the same queries.
- (B) Reproduced before the fix and fixed after: search 'lantern' (chips '1 in main / 2 in threads'), then 'old lantern' showing '0 of 60 loaded' / 'None match' must not still read '1 in main / 2 in threads' at any settled point; the chips hide or show the new query's counts, landed off the input path.
- Existing search, bucket, server-hits and reading checks stay green; no new per-keystroke layout read or full re-render is introduced.
- Selected validation passes locally before the owner-tier gate; the gate passes or pre-existing reds are proven on a clean github/main checkout.

# Notes

Owner-priority, fast-track: deployed on landing. Related: #1899 vibe-talk-search-bucket-chips-pinned (pinned chips; suspect and (A)/(B) surface), #1894 vibe-talk-search-extra-hits-off-by-default, #1901 vibe-talk-search-late-hits-shift-anchor, #1895 vibe-talk-link-search-semantics, #1898 vibe-talk-nested-slack-link-in-markdown-label, #1846 windowed lists.

## Resolution

[watermelon-full-preview] Landed 397ea3f5 on github/main. Root cause, measured not guessed: one `input` event ran the whole filter synchronously — `threadsWithHits`/link scans over every held message, seams, tallies, and place-keeping that read one layout rectangle per row. On an 816-row fixture a keystroke read 817–1644 rectangles (2016-row probe: 2017–4045, 248–1195 ms, long tasks to 1150 ms); the owner's trace (100k DOM nodes) shows the same shape in navigation (`changeChannelView` → `toPosition`, 349 ms layout) and in `footGap`/`getBoundingClientRect`/`scrollTop` self-time. Fix: over a long list (>200 rows) the input event now runs only cheap work — the query, a visibility pass over each row's once-folded cached text, O(1) bucket-chip retirement, and the mark timer — and the full filter change (place-keeping, thread/link lookups, seams, tallies, server ask) lands once, 150 ms after the last keystroke (`FULL_SEARCH_PAUSE_MS`, pinned in the tuning-bands table). Short lists keep the exact synchronous landing. The screen-noting (`rowsOnScreen`), the view-switch place reader (`readerPlace`), and the scroll anchor now measure only the screen's neighbourhood (binary search / from-the-foot), never the whole history. (B) was real as a transient: while a new query's debounced server answer was pending, the previous query's chips stood; they now retire inside the input event and fresh counts land with the answer. (A) profiled with chips on throughout (fixture + real-data copy). The 'On this screen' signpost stands down while a bucket chip stands, so it never peeks half-cut under the pinned panel. Real-data COPY profile (Slack fully off: dead-loopback `api_base`, fake placeholder credential, `backfill = "off"`, read-only channel; copy deleted after): 484 rows / 26,759 nodes loaded from the copy, search 6–12 layout reads per keystroke; Main→thread navigation still costs ~900 layout reads and a ~700 ms long task (full thread render + entry landings over a large DOM) — reported as the top remaining offender for a follow-up virtualisation slice, not fixed here. Gate: ba2e3971 red only on documents-backfilter/ios-keyboard-send/reload-place, exclusive rerun green (6 passed), each 3/3 solo — host-saturation flakes at load average 286; landed by replay over c59100f0 with the combined tree verified (page-3 641 pass, typing gate green).
