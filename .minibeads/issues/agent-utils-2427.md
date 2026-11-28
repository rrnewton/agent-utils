---
title: 'vibe-talk-windowed-default-on: windowed rendering on by default with a much smaller held window, because the owner''s page mounts about 80 screens of rows and freezes'
status: open
priority: 0
issue_type: bug
assignee: cl-flaketriage
labels:
- vibe-talk
- owner-ui
- performance
created_at: 2026-11-27T19:50:00-05:00
updated_at: 2026-11-27T21:10:00-05:00
---

# Description

Owner (2026-11-27, 7:24 PM ET): "The interface is generally being very laggy. I would like to load a LOT LESS context if that is what is slowing me down in the client."

Evidence comes from the server-side `page_freeze` lines the page's freeze watch reports (#2066), on the owner's page between 00:17 and 00:25 UTC on 2026-11-28. Only timings were read.

- 262 freezes in 8 minutes: median 523 ms, p90 2,046 ms, max 3,487 ms, about 220 s blocked in all.
- Every one has `windowed=false`.
- `scroll_height` is 107,166 px against a 1,322 px viewport, so about 80 screens of rows are held and rendered.
- Attributed callers: `Response.text.then` 69, `selectionchange` 27, `requestAnimationFrame` 16; 141 unattributed.
- The server is not the bottleneck.

Windowed rendering (#1846) mounts only the viewport plus a prefetch margin. It is off by default everywhere today.

- `windowedConfig` in `vibe-talk/web/src/core/settings/windowed.ts` reads the device setting `vibe-talk.voice.windowed-lists`, which defaults to off. The explicit-off key `vibe-talk.voice.windowed-lists-off` already exists.
- The default was held off "until the browser suite gains windowed variants", because flipping it reddens browser checks that expect every row mounted.
- When windowing is on, today's window is 1,200 px of prefetch on each side on a phone and 1,600 px on a desk, with a cap of 60 or 80 mounted rows.
- Independently, the data reach-back holds 300 messages on a phone and 5,000 on a desk (4 or 16 MB).

#2292 fixed the breaks known when windowing was on (the composer moving at the foot during arrivals, and a blank band after a mid-list jump). It also exercised search hits, jump-to-message and Back windowed with nothing broken. #2239 is still open: under estimated row heights, the windowed coverage shift can slide the slice off the row being read. It was found by a seeded fuzz; the deterministic cases pass.

This is owner-UI fast-track work (owner rule 370, owner request 381 on lag).

# Design

1. Measure first, at the owner's scale, through a local test instance of the real server (never the live server).
   - The fixture is a channel whose rows add up to about 100,000 px.
   - Arms: windowing off; on at today's window; on at a much smaller render window. Each arm also runs with a smaller data reach-back, because the owner asked to load less.
   - Desk 1280 and phone 412.
   - Workload: scrolling, typing in the composer, pointer and selection.
   - Measures: long tasks and long animation frames (count, total blocked ms, p90, max), rows mounted and scroll height, attributed with the freeze watch's own fields.
2. Check reading stability with windowing on, under owner rule 370.
   - The reading row stays put on load, trim, scroll, arrivals and own sends: at most 0.5 px, or at most 1.0 px where positions land on whole pixels.
   - The existing rule-370 and windowed checks run with windowing forced on.
   - #2239's fuzz seeds run at the new window, since a smaller window makes that path more frequent.
3. If both are clean, change the defaults in one small commit.
   - Windowing is on by default on phone and desk, and the explicit-off key stays the kill switch.
   - The render window (prefetch and row cap) shrinks to the measured values, and the data reach-back shrinks if the measurement supports it.
   - The Settings help text stays as it is: browser find reaches only shown messages.
   - Browser checks that assume every row mounted either pin windowing off explicitly, with a reason, or gain a windowed variant. The commit lists which.
4. If a stability trace fails, the failure is fixed through the existing seams (the scroll owner and pins) before the default changes, or this issue reports it and waits for the coordinator.

# Acceptance Criteria

- Before/after freeze numbers at the owner's scale, desk and phone: windowing off, on at today's window, and on at the chosen window. The chosen default cuts total blocked time and p90 freeze length substantially against windowing off.
- Windowing is on by default on phone and desk, with the chosen window. The explicit-off key turns it off.
- Rule-370 traces with windowing on: the reading row moves at most 0.5 px (or at most 1.0 px on whole pixels) on load, trim, scroll, arrivals and own sends, phone and desk. The #2292 composer-foot check stays green.
- Search hits, jump-to-message and Back work with the new default.
- Owner-UI screenshot matrix (light and dark, keyboard open and closed, phone and desk) with `BUILD.txt` naming the exact tree. The evidence goes to the coordinator before the owner gate.
- No wall-clock budget in any gating check (owner rule 346). Freeze measurements are evidence, not gate assertions; any new gating check counts mounted rows or other load-invariant facts.

# Notes

Filed by `cl-flaketriage` at the coordinator's request (2026-11-27). Owner P0, fast track.

Owner redirect (2026-11-27, 7:39 PM ET): "the old version of Windowing was completely broken when I turned it on so I turned it back off. we're looking for the new smart/incremental loading to become DEFAULT not maintaining multiple strategies. Also even on desktop I think 5000 messages is too large a target to load. The loading should be incremental and small."
- So windowed mode does NOT go on by default. The loading program (#2426) retires it.
- This issue became interim relief through a held-size parameter, with windowing off. The Design's windowing steps 2 and 3 are superseded by this note.
- The parameter was dead. The data reach-back (5000 desk, 300 phone) was never enforced: `reachBackExcess` has no caller. Nothing trimmed the page's in-memory rows during a session; only the saved device copy is cut, to 120 rows. The owner's 107,000 px page was session growth.
- The relief is therefore the residency layer of #2426 (its interface reviewed by the program's owner):
  - a newest window per Main and All view, 150 rows on a phone and 200 on a desk;
  - overridable by a new device key `vibe-talk.voice.resident-rows`. Not `data-reach-back`: the windowing switch saved that field's prefilled 5000.
  - Rows are given up only at the oldest edge, only while the reader stands at the newest with no search open, never a pinned row, and only down to a page boundary the page holds a cursor for, so the walk back has no hole.
- Measured on 2026-11-27, windowing off, single runs at load1 about 140-200. At the owner's scale (650 rows) against 150-200 rows, every freeze grows about linearly with held rows: the load draw, an arrival's redraw, a selection change, a keystroke's handler.

Related:
- #1846: windowed rendering.
- #2292: the windowed composer-foot fix (closed).
- #2239: the windowed coverage shift can lose the reading row (open).
- #2315: a keyboard-up arrival moves history (open, waiting on the owner).
- #2422: search cards wiped by a redraw (open).
- #2066: the page freeze watch whose lines are the evidence.
- #2426: the broader client loading program, owned by the client performance lane; files are coordinated through the coordinator.
