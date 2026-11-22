---
title: 'vibe-talk-shrink-anchor-policy: archiving or folding a message in fold-read mode must keep the reader oriented'
status: open
priority: 1
issue_type: bug
labels:
- vibe-talk
- owner-ui
created_at: 2026-11-22T05:50:00-05:00
updated_at: 2026-11-22T05:50:00-05:00
---

# Description

The owner: "I seem to experience a number of scenarios or archiving a message makes the screen jump when we're in the fold-read mode. This is a scenario where it's important for me to stay oriented because I'm trying to move to the next unread message and read it. I articulated this principle before where if the region above the thing disappearing is visible. I want it to stay put and the region below should move up to it. If I'm archiving something that's disappearing which goes off the top of the screen, then in that case the line that stays still should be the one that's still on the screen at the bottom of the thing disappearing. If the thing disappearing is so huge that it goes off the bottom and the top of the screen, the best we can do is try to make the center of the screen exactly at the seam between the prefix before The disappeared object and the suffix after it which now come together. Of course, if archiving message introduces a new fold line that itself takes a little space, so it's more like a collapse of a bigger object to a smaller object."

# Design

One shrink and remove anchor rule for archive (Done) in fold-read mode and for any collapse of a message to a fold line: (1) if the item's top is on screen, the content above holds still and the content below moves up; (2) if the top is off-screen above and the bottom is on screen, the content just below the item holds still and the content above moves; (3) if it spans both edges, the seam (where the content before and after meet, at the fold line) lands at the viewport's centre. It composes with the grow rule of #2076 in the same scroll owner, and with earlier archive anchoring work (#543, #1514, #1889, #1975, #2034, #2038).

# Acceptance Criteria

- Measured anchor deltas of 0 px (cases 1 and 2) and the seam at the centre (case 3), windowed on and off, phone and desktop; deterministic frame-based browser checks through the real router; marked before and after shots of one action each.

## Owner 308 addendum (2026-11-22 5:50 AM ET)

The owner, verbatim: "This is the kind of thing that should be numerical in variance that we can fuss test or check live in a debug mode" (that is: numerical invariance we can fuzz test or check live in a debug mode).

(a) One numeric invariant. State the anchor policy — the 307 three cases above plus #2076's top anchor for growth — as ONE numeric invariant: for every READER-caused height change, the chosen anchor's viewport Y moves at most 0.5 px (case 3: the seam within 0.5 px of centre). The documented exception: arrivals while idle at the bottom. Put it in one function the tests and the debug mode both call.

(b) A seeded fuzz test, deterministic and hot-machine safe (event-driven, no wall-clock budgets): random action sequences (archive or Done in fold-read, fold, expand, select, quote, reply box, arrivals), windowed on and off, phone and desktop, asserting the invariant after each step and printing the seed and step on failure. Keep the gated run small (a few fixed seeds); allow a long opt-in run.

(c) An opt-in live debug mode (a setting or URL flag, default off, page-only, read-only) that checks the invariant on every layout change and records violations (action, anchor, delta px, view, windowed state) to the console and to the freeze telemetry channel from #2066 (the freeze-report route; timings only, never content).

Landing split: (a) and (b) land with the 307 fix; (c) lands as the next commit.

# Notes

Filed by the coordinator; the owning lane refines this file as it works.

## Resolution in progress (mu-shrinkanchor, 2026-11-22)

Cause: archive (Done) in Fold read never went through `preservingCollapse`/`beginUserChange` — the #2076 hold in `createScrollKeeping` (`scrollkeep.ts`). It redrew under the generic `holdingMessage` first-row hold (`doneact.ts` → `refreshAfterInboxChange`/`renderDoneChange`), with later same-press writers free to move it (bookmark hold, deferred reply-arrow measure, cross-column redraw, windowed re-slice, foot keepers, and `donekeys`' immediate next-unread `bringIntoView`). Before, frame-measured in Fold read (Done button, rAF to settle+30): content above moved 79.0px (412) / 79.3px (1280), content below 42.9 / 43.4px, spanning item never centred; windowed on = same.

Fix (f3f1e215): one row's Done/put-back routes through `preservingShrink`/`preservingShrinkAsync` in the same scroll owner, composed with #2076: three cases from `anchorpolicy.shrinkCase`, anchor re-asserted every frame until the act and its landing renders settle (45 still frames, corrections capped), opening the user-caused hold; the next-unread move waits for `shrinkSettling` and lands as its own explicit move. (a) `anchorInvariantHolds` (≤0.5px; seam within 0.5px of centre; arrivals idle at the bottom exempt) is the one function tests and debug call. (b) Seeded fuzz in `tests/shrink_anchor_browser.py` (gated seeds 1,2,3 × 14 steps, windowed on/off, phone and desktop; long opt-in `--fuzz-steps 200`). After: cases 0.00–0.28px every frame, seam 0.11px off centre, next-unread shrink held then landed.

Debug mode (850ab307, own commit): opt-in `?anchordebug=1` or held `vibe-talk.voice.anchor-debug`, default off, page-only, read-only; settled shrinks judged by the same invariant, violations to console and the #2066 freeze-report route (timings only).
