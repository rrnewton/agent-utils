---
title: 'vibe-talk-read-row-moves-start-stop: the row being read still moves as a read starts (the follow centres it) and as it stops (the browser clamps as the frame grows back)'
status: open
priority: 1
issue_type: bug
labels:
- vibe-talk
- owner-ui
- read-aloud
created_at: 2026-11-27T05:21:00-05:00
updated_at: 2026-11-27T07:55:00-05:00
---

# Description

Found by the per-frame traces of #2351 vibe-talk-read-strip-foot-reader-moves (the read_aloud_blocks_browser walk at 393x852 and 375x812, dark and light, a read started, paused, resumed and stopped with the list at its end). Once #2351 stopped the read strip's measure from taking the reader to the new foot, two other movements of the row being read remained. They have different causes and different owners, so they are kept apart here.

**(a) Read start: the read-follow centres the row being read (an owner question).** The read's start calls the follow on the row (no dead zone), and the first sentence light calls it again with a 24 px dead zone. The follow centres the row in the middle third of the list's port, above the reading notice and the read strip (#1866). At 393x852 it glides the row 37 px up (smooth, at most 20 px in one frame); at 375x812, 14 px. This is the deliberate behaviour the owner asked for in #1587/#1588 (keep the highlighted part in view; a jump centres the row, #837). Before #2351 it was hidden: the strip's 47 px jump happened to land the row inside the follow's dead zone. It conflicts with R12 (owner request 370: nothing moves the row being read).

**(b) Read stop: a clamp drop (not design).** When the read stops, the strip goes and the list's frame grows back by the strip's height (47 px at 393x852). A reader the follow had moved further down than the list's overflow without the strip (scrollTop 37 against 5 px of overflow at 393x852) is clamped by the browser, and the row drops 32 px in one frame. Nothing asked for this; it is a clamp jump, and rule 370 applies directly.

# Design

- (a) No code. The owner decides between the follow's centring (#1588) and R12: for example, whether a read's start leaves a fully visible row where it is, and the follow moves only once the highlighted part leaves the port or its comfortable band.
- (b) Hold the row being read with room, as #2346 holds a landed search row. When the strip's measure grows the frame back, take the reading row's top before the frame changes; if the browser's clamp would move it, hold it there with room at the list's foot (the scroll owner's row hold). That room is cleared by the reader's next scroll or by the jump. Channel list only; the call transcript keeps its own contract (#431). No owner decision needed.

# Acceptance Criteria

- (a) The owner's answer is recorded here; code only after it, in a follow-up issue or here.
- (b) Per-frame traces at 393x852 and 375x812, light and dark, of a read started, paused, resumed and stopped with the list at its end: the read row's top moves <= 0.5 px across the stop, with #2351's build as the negative control (32 px at 393x852) and a JSON sidecar.
- (b) read_aloud_blocks_browser stays green; shots after the stop at 393 and 375 with BUILD.txt; the advisory design review.

# Notes

Filed at the coordinator's request from #2351's traces (issue file only). Also seen in those traces, for the record: at 393x852, with the reader 5 px above the list's end (inside the slack), the chips row's own follow (#1185) takes them the 5 px to the foot at the press, and they come back 5 px as the strip appears (about 3 frames). Related: #2351, #2346, #2354, #1587, #1588, #1866, #837, #1185, rule 370 (R12).

## Part (b) landed

[opus 5.5, cl-signpost3] Part (b) landed as 1110ad92 (gated head 1d968c39, owner gate full: 341 steps passed. It was PARTIAL only from 7 delegated-live-scope dagrun/cpuset cross skips; per-landing waiver from the liaison).

- **Hold.** When the read strip's size changes in a channel list, its measure takes the reader's scroll anchor before the frame changes. If the browser's clamp moved it, the measure holds it with room at the list's foot (scrollKeeping.holdRowAt, as #2346 does).
- **Veto.** chipsFollowFoot and the list's dock keeper now leave a reader standing in held room where they are (scrollKeeping.holdsRoom). Real-browser traces found both reclaiming the room at the next rendering and letting the clamp drop the row.
- **Traces.** At 393x852 and 375x812, light and dark, across the read stop: 0.0 px. Base 39c0d887 dropped 32 px at 393.
- **Page test.** Three negative controls (no hold, no chips veto, no dock veto).
- **Liaison checks.** Desk and keyboard-open cells unchanged. search_unmatched, floating_chip_overlap and label_chip_tap 3/3. A dock growth over a search or tap hold is a no-op: the filter-hold checks come first. An arrival or an own send after the stop moves nothing and is offered by the chip, and the held room is absorbed.

Part (a), the read-follow's centring at a read's start against R12, stays open for the owner. Related follow-up: #2375 (the reading-mode-off toast over a sent row).
