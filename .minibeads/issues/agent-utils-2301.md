---
title: 'vibe-talk-ui-review-findings-11-26: fix the five page nits from the first design-review pass'
status: open
priority: 1
issue_type: task
labels:
- vibe-talk
- owner-ui
created_at: 2026-11-26T11:35:00-05:00
updated_at: 2026-11-26T11:35:00-05:00
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
