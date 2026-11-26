---
title: 'vibe-talk-root-has-rules: no stylesheet rule anchored at the page root makes a small show or hide re-scan the whole document'
status: open
priority: 1
issue_type: bug
labels:
- vibe-talk
- performance
created_at: 2026-11-26T04:30:00-05:00
updated_at: 2026-11-26T04:30:00-05:00
---

# Description

Found while root-causing `#2277 vibe-talk-key-scroll-freezes-11-26`, as a secondary cost beside its main cause.

The page's stylesheets hold 115 rules with `:has()`, many anchored high in the page (`:root:has(…)`, `body:has(…)`, `#frame-body:has(…)`, `#screen-main:has(…)`, `#dock:has(…)`) with arguments that look anywhere below the anchor (`:root:has(#pane-docs:not([hidden]) #docs-list-view:not([hidden]))`). The browser can only check such a rule by searching the anchor's whole subtree, which is the whole document, so any change that might affect its argument makes it do that again. Showing or hiding one small control is enough: the page does this on its own while the reader types and scrolls (the keyboard's corner Send on scroll settle, the action menu during the bar's width measure, the Newest chip, the error banner).

Measured on a local server running the deployed build against a copy of a busy channel, at phone size in the flat All view (one hidden toggle plus one forced style read, median):

| Rows | Elements | Unthrottled | CPU throttled 4x |
|---:|---:|---:|---:|
| 192 | 11,669 | 6.6 ms | 59 ms |
| 384 | 21,067 | 11.7 ms | 90 ms |
| 768 | 39,863 | 21.2 ms | 189 ms |

With every `:has()` rule disabled, the same toggles cost 0.0-0.2 ms. The cost grows with the length of the list, and the page makes several such toggles per scroll and per refresh.

# Design

1. Replace the root-anchored `:has()` rules with state the page already knows: the code that shows or hides the element (a pane, a panel, the keyboard's Send, a reply box) also sets a `data-*` flag on the anchor (`:root`, `#frame-body`, `#screen-main`, `#dock`), and the rule reads the flag. One owner per flag, set and cleared at the same place the element's visibility changes.
2. Keep `:has()` only where it is local: the anchor is a small container and the argument is a direct child (`> …`), so the browser checks only that container's children.
3. A static check in the page's style tests refuses new `:has()` rules anchored at `:root`, `html`, `body`, `#frame-body`, `#screen-main` or `#dock` whose argument is not a direct child.

# Acceptance Criteria

- No stylesheet rule anchored at the page root, body or the main frame uses a descendant `:has()` argument; the static check refuses one.
- With 200+ rows at phone size, a hidden toggle of each control the page toggles while typing and scrolling restyles only a bounded number of elements (counted from the trace's style-recalc element counts, never wall time).
- Every view that the replaced rules styled looks the same in the screenshot matrix (light and dark, phone and desktop, keyboard open and closed).

# Notes

Follow-up of `#2277` (coordinator decision, 2026-11-26). Measurements and the probe script came from `#2277`'s root cause. An older note in the control-pane stylesheet (`#532 shift-o-latency`) already fixed one rule of this kind whose last part named no class, id or attribute.
