---
title: 'vibe-talk-windowed-coverage-shift-loses-reading-row: in windowed mode the coverage shift can slide the slice off the row being read under estimated heights'
status: open
priority: 1
issue_type: bug
labels:
- vibe-talk
- scroll
- windowed
created_at: 2026-11-25T09:10:00-05:00
updated_at: 2026-11-25T09:10:00-05:00
---

# Description

[claude-opus-5-5] With windowed lists on (an opt-in device setting, off by default), the windowed renderer's coverage shift (`renderWindowed` in `web/src/view/channel/readcore.ts`, the `#1985` "coverage shift" that moves the slice when the mounted rows do not fill the reader's band) can slide the slice off the row being read. The slice is priced from estimated row heights; when the mounted rows end short of the band, the shift moves the whole slice down by the estimated number of groups, unmounting the first rows, including the row on screen. The reader then sees spacer where their row stood, and no anchor nudge can hold a row that is no longer mounted.

Found by `vibe-talk.check.shrink-anchor-browser`'s seeded fuzz while landing #2230 vibe-talk-reading-row-stays-put (owner 370: no arrival follows the foot), which changes the fuzz trajectory: arrivals now pile up below a reader near the foot instead of taking them to it.

Evidence (seeded fuzz, `tests/shrink_anchor_browser.py --fuzz-steps 14`):

- On the base (main before #2230), seeds 1-3 pass, but seed 7 fails: "fuzz seed=7 step=5 fold 209: touched top moved 186.00px". The fuzz is fragile on the base too; seeds 1-3 passing there is trajectory luck.
- With #2230, phone-412 windowed: "fuzz seed=3 step=5 arrival: the reading row was lost" (and at step 13 after a partial guard); seed 8 fails like the base's seed 7 ("fold 205: touched top moved 186.00px"). Desk-1280 windowed: "fuzz seed=2 step=10 archive 211 case2: below moved 314.56px". The non-windowed fuzz (phone-412 and desk-1280, seeds 1-3) and every deterministic case pass.
- A trace of the slices at seed 3 step 5 (phone-412 windowed): the first render's slice 1..9 held the reading row (group 4); the coverage shift forced 5..13, dropping it.
- A guard that grows the shifted slice to keep the mounted anchor (instead of sliding past it) fixed step 5 but not step 13. There a later render found no mounted row on screen at all (the anchor came back null), so the next shift had nothing to keep: a deeper estimated-height issue than the slide alone.

# Design

- Make the coverage shift keep the row being read: grow the slice toward the gap (within the row cap) rather than sliding it, and carry the original anchor through nested shifts and the converge re-slice.
- Find why a render can end with no mounted row in the reader's band (anchor null), and make the slice always cover the band before any shift is priced.
- Then bring the windowed fuzz halves back into gating, with seeds 1-3 and the base's failing seed 7 (and #2230's seed 8) green.

# Acceptance Criteria

- `tests/shrink_anchor_browser.py --fuzz-seeds 1,2,3,7,8 --fuzz-steps 14` passes at phone-412 and desk-1280, windowed on and off.
- The `--fuzz-skip` entries added by #2230 are removed from the gated step in `tests/dag.d/179-shrink-anchor-browser.yaml`.
- A page or browser test pins the shift: an arrival or fold that ends the slice short leaves the row being read mounted at its top (within 1px).

# Notes

Out of gating meanwhile (owner rule 346: fragile on the base too): #2230 skips the failing windowed fuzz runs of the gated shrink-anchor step, each with a named `--fuzz-skip` line (the step's command lists them). The two skipped runs are phone-412 windowed and desk-1280 windowed. The non-windowed fuzz and every deterministic case, windowed included, stay gated.

Not in the owner's path: the owner's live page clients report windowed=false in their freeze reports, so this opt-in mode is not what the owner reads with.
