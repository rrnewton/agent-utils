---
title: 'vibe-talk-placegrip-measures-ch-every-resize: the reading-width handle re-measures 1ch with a probe span on every resize of the list'
status: open
priority: 1
issue_type: bug
labels:
- vibe-talk
- performance
created_at: 2026-11-26T06:20:00-05:00
updated_at: 2026-11-26T06:20:00-05:00
---

# Description

Found by the attribution for `#2277 vibe-talk-key-scroll-freezes-11-26`, as one of the forced style recalculations behind the night's key and scroll freezes.

`placeGrip` (`vibe-talk/web/src/view/settings/screen.ts`) places the reading-width handle. A `ResizeObserver` calls it whenever `#scroll-area`, `#pane-voice` or `#pane-discord` changes size, and the channel pane changes size every time its content's height does: a row arriving, folding, unfolding or being redrawn. Each call runs `measurePerCh`, which reads the pane's computed font, appends a hidden `<span>` of ten zeros to `document.body`, reads its `getBoundingClientRect()` and removes it. Appending to the body and reading layout forces a style recalculation of the document. With the root-anchored `:has()` rules of `#2283 vibe-talk-root-has-rules` still in the stylesheets, each forced recalculation searches the whole document.

Measured by the `#2277` lane on a local server running a page build with about 750 rows (about 38,600 elements), phone size, CPU throttled 4x, from the trace's forced style and layout attributed by JavaScript caller:

| Long frame | Forced style/layout in `getBoundingClientRect <- measurePerCh <- placeGrip` |
|---|---:|
| worst long animation frame (871 ms) | 125 ms |
| worst long task (635 ms) | 190 ms |

The figure `measurePerCh` answers changes only when the pane's font (family, size, weight, letter spacing) changes, not when the list grows. The handle's left edge and whether it shows depend on the column's width, which also does not change when rows arrive.

# Design

1. Cache the measured px-per-ch keyed by the inputs it depends on (the pane's font family, size, weight and letter spacing, read once per change of those inputs), and measure again only when that key changes: a type-size change in Settings, a theme or font change, a zoom. `placeGrip` and `pixelsPerCh` read the cached figure.
2. `placeGrip` does nothing when the observed boxes' widths are unchanged since its last run (a height-only resize of `#pane-discord`, which every arrival causes, places nothing new). Track the last widths the observer reported, from the `ResizeObserver` entries rather than fresh layout reads.
3. Where a measure is still needed, take it without appending to `document.body` in the middle of a frame that has pending style changes: for example a probe element kept in the page, hidden, whose font follows the pane, read in the same pass as the other layout reads.

# Acceptance Criteria

- A counting browser check (no wall-clock bound; rule 346): with a channel of 200+ rows at 412x915 and 1280x800, N live arrivals that change only the list's height cause zero `measurePerCh` probes and zero handle re-placements, counted through a test hook or a `MutationObserver` on `document.body` for probe spans; a change of the reading type size causes exactly one measure.
- The handle still appears, hides and moves exactly as before on a window resize, a Discuss dock, undock or maximize, and one or two columns (the page suites that cover `width-grip` and `two_column_browser.py` stay green).
- In the `#2277` attribution trace (same page build and row count), no forced style/layout is attributed to `measurePerCh <- placeGrip` during arrivals or scrolling.

# Notes

Related: `#2277 vibe-talk-key-scroll-freezes-11-26` (the attribution that found it), `#2283 vibe-talk-root-has-rules` (why each forced recalculation costs the whole document), `#2282 vibe-talk-client-flow-analysis` (the page's trigger map), `#373 two-column-full-width` (why the handle is hidden in two columns).
