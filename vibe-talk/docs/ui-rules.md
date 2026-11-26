# vibe-talk UI rules — load this whole file before any page UI work

Every lane that touches the vibe-talk page (`vibe-talk/web/`) loads this file
first. It is short on purpose. Each rule is one imperative line, a one-line
reason, and a reference: M3 = Material 3 (m3.material.io), HIG = Apple Human
Interface Guidelines (developer.apple.com/design/human-interface-guidelines),
Local = a standing rule for this page. Rule numbers (R1…) are how a design
review cites a violation. Filed under #2298 vibe-talk-ui-rules-and-design-review.

Platform first, then product: where M3 and HIG agree, follow both; where they
differ, take the stricter value (48 satisfies 44). A deliberate exception names
the rule it breaks in the lane's handoff.

## Button groups and alignment

- R1. Reserve end (trailing-edge) alignment for dialog actions; never use it for page-level controls.
  Reason: trailing alignment is the dialog-action convention; on a page it strands controls away from the reading edge.
  Ref: M3 Buttons, Alignment ("Align text buttons to the trailing edge of dialogs"); M3 Dialogs ("Buttons are aligned to the trailing edge of the dialog").
- R2. On a narrow (compact/phone) width, lay a page-level button group out as equal-width columns filling the content width, or as full-width stacked rows; on a wide width it may shrink to its labels or centre as a group.
  Reason: compact layouts have one content column, on 16px margins; a group that neither fills, stacks nor centres reads as unfinished.
  Ref: M3 Buttons, Layout & spacing (group treatment changes with available width); M3 Layout (16dp compact margin); HIG Layout (16pt compact margin); #2297 vibe-talk-settings-header-layout applies this to the Settings header.
- R3. Give a screen one primary (filled/prominent) button; make every other action visually quieter.
  Reason: competing filled buttons destroy the scan order.
  Ref: M3 Buttons, Usage (emphasis by container); HIG Buttons (prominent style sparingly).

Worked example (the anti-pattern): the Settings header buttons
"Expand all / Welcome / Open help" once sat end-aligned on a phone — neither
filling the column nor centred (breaks R1, R2). Compliant: three equal-width
columns filling the settings column, order and actions unchanged (#2297).

## Text economy and truncation

- R4. Write control labels in a few words, usually a short verb phrase ("Open help", "Save draft", "Send"); never put explanatory prose in a control.
  Reason: controls are scanned, not read; prose belongs behind a tap.
  Ref: HIG Buttons ("Create short labels", one line, no ending punctuation); Local: no verbose prose in the UI.
- R5. Show status as a short chip, badge or count, with detail one tap away; never as a sentence of page chrome (build ids, server state, hints).
  Reason: a sentence competes with content and wraps unpredictably; a chip is scannable and tappable.
  Ref: M3 Chips (compact elements representing status/attributes, appear dynamically); Local: no verbose prose in the UI.
- R6. Truncate with a single-line ellipsis and keep the full value reachable (tap, tooltip, accessible label); never cut a value silently mid-stream.
  Reason: silent truncation hides information with no way back.
  Ref: M3 Lists (specify overline/headline line limits, truncate with ellipsis); HIG Labels (middle truncation keeps the distinguishing end visible).
  Note: counted truncation ("(~190 more words)") satisfies this rule — it says how much is hidden.
- R7. Keep a label-plus-value line on one line: the label never wraps, the value ellipsizes, and a trailing unit or port stays whole.
  Reason: a wrapping label breaks the column edge every other row holds.
  Ref: M3 Lists (one-line list item); HIG Labels.
- R8. Use one glyph per meaning everywhere: one dismiss glyph (pick × or ✕ and use it everywhere), a chevron to disclose, the ≡ grip to drag.
  Reason: two glyphs for one job, or one glyph for two jobs, teach the reader the wrong tap target.
  Ref: HIG Icons (familiar, consistent symbols); M3 Icon buttons (one recognisable action per button); Local: see Deliberate idioms below.

## Touch targets, spacing, placement

- R9. Make every touch target at least 48×48 CSS px (the hit area may exceed the visible glyph), with at least 8px between adjacent targets.
  Reason: smaller or crowded targets cause mis-taps no styling can fix.
  Ref: M3 Accessibility (48×48dp minimum, 8dp between targets); HIG Accessibility (44×44pt minimum, ~12pt padding around bezeled elements). 48 satisfies both.
- R10. Take every spacing value from the 4/8 scale (4, 8, 12, 16, 24, 32…) and hold 16px side margins on phone widths; invent no one-off gaps.
  Reason: a shared scale is what makes separately built screens line up.
  Ref: M3 Layout (4dp baseline grid, responsive spacing scale 8–32dp, 16dp compact margin); HIG Layout (16pt compact / 20pt regular margins).
- R11. Place on whole pixels: positions and sizes land on integer CSS pixels, and geometry traces assert movement thresholds of at most 0.5px (1.0px only where a whole-pixel step is the claim).
  Reason: fractional placement blurs edges and makes "did it move?" unanswerable.
  Ref: Local: whole-pixel placement (trace thresholds ≤0.5px, or ≤1.0px on whole pixels).

## This page's standing rules

- R12. Never move the row being read; let growth extend below it.
  Reason: the reader's place is sacred, including while they type or new content arrives.
  Ref: Local rule 370 (the row being read never moves; growth extends below).
- R13. Give each composer exactly one Send.
  Reason: two sends for one draft is a coin flip at the moment of commitment.
  Ref: Local: one Send per composer. (Two composers on screen — channel + Discuss — each keep their own single Send, and only the focused composer's Send is prominent; see R3.)
- R14. Match dark-mode composer surfaces to the message cards (same background step, same border treatment).
  Reason: a composer that glows or sinks against its cards reads as a different app at night.
  Ref: Local: dark-mode composers match the cards. Judge by sampling pixels in the screenshot matrix, not by token names.
- R15. Reuse the page's existing surfaces (cards, chips, sheets, composers) for new chrome; introduce a new surface only when none fits, and say so in the handoff.
  Reason: every new surface is a new convention the reader must learn and the next lane must guess at.
  Ref: Local (consistency); HIG (consistency with the platform and the app itself).

## Deliberate idioms (do not flag)

These look like violations of generic guidance and are intentional here. A
reviewer who flags one has misread the surface; lanes keep them as they are.

- The ≡ in Discuss and Timeline headers is the drag grip (#1584), not a menu and not a disclosure control; the chevron beside it discloses. Both glyphs together are correct.
- Each composer pairs its one Send with a ↵ new-line key. The ↵ key inserts a line break; it is not a second Send (R13 counts Send buttons only).
- The fitted composer (#2277) sits corner-less in its slot by design; do not flag its missing corner ornaments or its tight fit as crowding.
- Anything the owner explicitly asked for wins over generic guidance in this file; cite the request in the handoff instead of flagging or "fixing" it.

## Evidence for every UI change

- R16. Capture the screenshot matrix before handoff: light/dark × phone/desktop × keyboard open/closed, with Discuss open, plus the changed surface in each of its states.
  Reason: most layout defects above are invisible in code review and obvious in one screenshot.
  Ref: Local: screenshot matrix (light/dark × phone/desktop × keyboard open/closed, Discuss open).
- R17. State, beside the matrix, what screenshots cannot prove for the change (motion, timing, focus order, live-update behaviour) and what evidence covers it instead (trace, test, paired frames).
  Reason: a still that cannot show the claim must not be offered as its proof.
  Ref: Local (advisory design review, #2298): reviewers list violations with crops and name the cannot-judge set.

## References

- Material 3: Buttons, Dialogs, Chips, Lists, Layout, Accessibility — https://m3.material.io/
- Apple Human Interface Guidelines: Buttons, Labels, Icons, Layout, Accessibility — https://developer.apple.com/design/human-interface-guidelines
- Anthropic frontend-design skill (process and copy discipline; a lane may load it in addition to, never instead of, this file) — https://github.com/anthropics/skills/tree/main/skills/frontend-design
