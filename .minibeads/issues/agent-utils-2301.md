---
title: 'vibe-talk-ui-review-findings-11-26: fix the five page nits from the first design-review pass'
status: open
priority: 1
issue_type: task
labels:
- vibe-talk
- owner-ui
created_at: 2026-11-26T11:35:00-05:00
updated_at: 2026-11-26T12:25:00-05:00
---

# Description

The first advisory design-review pass (#2298 vibe-talk-ui-rules-and-design-review) read the after-landing screenshot matrices of #2277 vibe-talk-key-scroll-freezes-11-26, #2279 vibe-talk-post-facto-labels and #2285 vibe-talk-labels-browser-tapped-message-jumps against vibe-talk/docs/ui-rules.md. Twelve findings stood after crop re-inspection. Triaged 2026-11-26, five are to fix in this issue (rule numbers are the rules file's):

1. Composer corner overlap (R10 spacing): in light--android-412--77-composer-keyboard-open.png the Save draft chip and the new-line key stack and overlap in the composer corner.
2. Two dismiss glyphs (R8 glyphs): label chips dismiss with × while sheets dismiss with ✕ (dark--android-412--151-label-picker-keyboard.png against the sheet close). Pick one glyph and use it everywhere.
3. Picker placeholder prose (R4 text economy): the label picker field reads "Find or add · # for a hashtag label" (light--android-412--150-label-picker-menu.png). Shorten to "Find or add" and show the # syntax as a separate hint.
4. Bare count (R5 status): each label row carries an unexplained "1" at its end (light--android-412--150-label-picker-menu.png). Label what it counts or drop it.
5. Gold section header (R5 status): the picker's "Pinned" header is gold while every neighbouring header is neutral, with no stated meaning for the colour. Use the neutral header colour.

Explicitly not in this issue, by the same triage. Dropped: the dark-composer mismatch finding (fixed by #2294 vibe-talk-dark-composer-black, landing) and the "≡ + ▼" header finding (the ≡ is the Discuss drag grip from #1584, a deliberate idiom now listed in the rules file, not a disclosure glyph). Deferred to the owner with no change yet: touch-target and gap sizes (the SOUND tile at about 33 CSS px, small pills, about 5 CSS px gaps, against the 48 px / 8 px guidance in R9/R10), because density is the owner's taste to set.

# Design

Page-only fixes, one surface at a time, reusing existing components (R15). The composer corner keeps both controls: lay Save draft and the new-line key out so they never overlap at 412 px or 1280 px in either theme, on the 4/8 spacing scale, without moving Send or changing what either control does; the corner-less fitted composer (#2277) and the Send + new-line pairing are deliberate idioms and stay. The dismiss-glyph fix is a sweep: choose the glyph the majority of surfaces already use, change the minority surfaces, and add no new glyph. The picker changes are copy and colour only: placeholder, hint placement, count label (or removal), and header colour token; picker behaviour, row order and selection state do not change. Whole-pixel placement (R11) applies to every moved control. Nothing here touches the server, the contract, or message rendering.

# Acceptance Criteria

- In the screenshot matrix (light and dark, phone and desktop, keyboard open and closed, Discuss open; R16), the composer corner shows Save draft and the new-line key with no overlap and spacing from the 4/8 scale; a geometry check or measured crop in the evidence shows the gap.
- Every dismiss affordance in the matrix uses the same glyph; a search of the page sources finds no remaining use of the other glyph for dismiss.
- The picker field placeholder is "Find or add" (or shorter), the # syntax hint is visible without typing, each label row's count is labelled or absent, and the Pinned header uses the neutral header colour, in both themes.
- A fresh advisory design review over the new matrix (the standing step from #2298) reports no R4, R5, R8 or R10 violation on the touched surfaces; the deliberate idioms (drag grip, Send + new-line pair, fitted composer) are unchanged.
- Touch-target sizes and gap sizes are byte-for-byte unchanged in the touched surfaces (the deferred owner decision is not smuggled in).

# Notes

[watermelon-full-preview] Filed 2026-11-26 from the #2298 vibe-talk-ui-rules-and-design-review pilot (report §6, findings §6.2 and §6.3; two further pilot findings were withdrawn on re-inspection and are not here). Owner-UI fast-track applies when the page-nits lane picks this up. If the owner later settles the density question, file the target/gap work as its own issue rather than widening this one.

## More findings from the #2297 review (2026-11-26)

The advisory design review of #2297 vibe-talk-settings-header-layout (screenshot matrix 73-77 plus Settings states 155/156, light and dark, android-412 and laptop-1280) also found these. All of them predate #2297, which does not change them. The coordinator added them here. The dark-composer finding (R14) is left out because #2294 vibe-talk-dark-composer-black fixes it.

6. Channel Settings "?" (R9 targets): on a phone the "?" beside the channel select is about 24 CSS px across and about 8 CSS px from the select (shots 155/156, android-412), so a 48 px hit area would overlap the select. This is part of the touch-target sizing question deferred to the owner above. Record it there and do not resize it under this issue.
7. Focused composer, filled Send (R13 with R3): with the Discuss composer focused on a phone (76-discuss-keyboard-open, both themes), the filled, prominent Send belongs to the unfocused channel composer, and Discuss's own Send is the quiet one. Only the focused composer's Send should be prominent.
8. Pace and Read both filled (R3): Pace and Read are both filled primary buttons in the phone control bar (73-75), and the channel Send is filled too, so up to three filled buttons share one screen.
9. Discuss status sentence (R5): after a summary, the Discuss header reads "Read 7 unread of 7. Older messages weren't fetched, so this covers the newest part only — summarize again later to include more of them.", three lines on a phone (74-discuss-summary). Show a short status with the detail one tap away.
10. Settings card gap (R10 spacing): the Settings section cards are about 10 CSS px apart, which is off the 4/8 scale, while the gap from the header buttons to the first card is 12 (155/156, both sizes).
