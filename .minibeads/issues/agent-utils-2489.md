---
title: 'labels-pill-hides-pinned-word: on a desk the Labels button with Pinned in force is an empty gold pill, the word "Pinned" not drawn'
status: open
priority: 3
issue_type: bug
assignee: cl-filternote
labels:
- vibe-talk
- search
- owner-ui
created_at: 2026-11-28T13:38:00-05:00
updated_at: 2026-11-28T13:38:00-05:00
---

# Description

Found by the advisory design review of `#2483` labels-redraw-drops-focus (its after-Enter still at 1280x800, light and dark), outside that change. On a desk, once Pinned is chosen (from the Labels drop-down or Shift+P), the search bar's Labels button shows the pin, about 74 px of gold fill, and the chevron: the word "Pinned" is not visible. The button's markup carries the word (`syncButton` writes "Pinned" into `.labels-filter-word`, and the aria-label says "Labels: Pinned selected"), and the desk stylesheet shows the word (only phones under 480 px hide it on purpose), so a sighted desk reader sees a filled pill that names nothing.

Likely cause, not yet proven: `#labels-button[data-active-label="pinned"]::before` draws the gold pill as an absolutely positioned layer; positioned content paints above the button's non-positioned children, so the pill may cover the word while the pin and chevron show for reasons of their own. A trace of the painted layers (or toggling the pseudo-element's `z-index`) should confirm it before any fix.

# Design

- The pill stays behind the button's content: the word, the pin and the chevron all stand on it (for example `z-index: -1` on the pseudo-element inside a button that makes its own stacking context, or the pill drawn as the button's own background), with the word in the pin ink as the stylesheet intends.
- No change on a phone (the word stays hidden there by design) and no change to a hashtag label in force (accent text, no pill).

# Acceptance Criteria

- A browser walk (fixture server, never the live server) that fails on main and passes with the fix: at 1280x800 with Pinned in force, the word "Pinned" inside the Labels button is painted (its box visible and the pixels under its text differ from the pill's fill), in light and dark; at 412x915 the button is still the pin square.
- Owner-UI matrix: desk and phone, light and dark, keyboard open and closed, Discuss open, before and after, with BUILD.txt; the advisory design review.

# Notes

- Filed at the coordinator's request (number assigned by the coordinator) from the `#2483` design review; assigned to cl-filternote. The still and crop: the `#2483` evidence (after/desk-light-4d-dropdown-keyboard-enter.png and its crop of the bar).
