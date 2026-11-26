---
title: 'vibe-talk-refreshing-pill-covers-message-text: the floating freshness pill, now the only sign a background re-read is out, hides message text under it'
status: open
priority: 2
issue_type: bug
assignee: cl-arrival
labels:
- vibe-talk
- owner-ui
created_at: 2026-11-26T16:40:00-05:00
updated_at: 2026-11-26T16:40:00-05:00
---

# Description

Since `#2291 vibe-talk-top-anchor-arrival-27px-race`, a background re-read over rows on screen (poll, live arrival, coming back to the page) no longer puts the in-flow "Loading messages…" line above the history; the floating freshness pill ("Live · updated HH:MM · refreshing…", `#channel-freshness`) is the only sign the read is out. The pill floats over the top of the list on purpose (`#1320`, `#1862`: it reserves no room, so no row moves when it appears), but it is drawn over the first visible card and hides product text there. Its text and placement did not change in `#2291`; the same overlap shows whenever it stands (Saved, Refresh failed, refreshing).

Found by the advisory design review of `#2291`'s screenshot matrix (rules file `vibe-talk/docs/ui-rules.md`, `#2298`), measured on the held re-read state:

- Desktop 1280x800, light and dark (DPR 1): the pill at x 205-435, y 13-39 CSS px (230x26) hides the top 3 CSS px (ascenders and cap tops) of the first card's first body line along 230 px.
- Phone 412x915 with the keyboard open (579 CSS px tall, DPR 2.625): the pill at x 91-321, y 13-38.5 CSS px covers the tail of a partly visible card's counted-truncation label, so only "(~80 mor" is readable (about 54 CSS px hidden).
- Phone with the keyboard closed: the pill comes within 1-2 CSS px of the first card's timestamp.

The review cites R5 (status as a short chip, detail one tap away; status must not compete with content). `ui-rules.md` has no rule that floating chrome never covers message text; this issue may add one.

# Design

Keep the pill out of the list's flow (no row may move when it appears, changes words or goes: `#1320`, rule 370), and keep it off message text:

1. While a read is out, say so with a short chip ("Refreshing…") in a slot that holds no message text (for example the top-right gutter beside the search button on phone, or a busy state on an existing bar control), with "updated HH:MM" one tap away (R5).
2. Or give the list a top inset equal to the pill's band only while the pill stands, applied through the scroll owner's holds so the row being read keeps its top (rule 370), never as an in-flow element.
3. Add the missing rule to `ui-rules.md` if the owner agrees: floating chrome never covers message text.

Pick one with the liaison before building; capture the matrix (R16) in the held, rest and landed states.

# Acceptance Criteria

- In the held background re-read state, on phone (keyboard open and closed) and desktop, light and dark, no product text sits under the pill (pixel check in the screenshot matrix).
- No row moves when the pill appears, rewords or goes (per-frame trace, within 0.5 px).
- The pill still says a background read is out for its whole duration.

# Notes

Related: `#2291` (removed the in-flow line for background reads, which made the pill the only sign), `#1320` and `#1862` (the pill floats and reserves no room), `#1837` (the current pill dismisses itself), `#2298 vibe-talk-ui-rules-and-design-review` (the review that found this; rules gap). The review's crops and the before/after shot matrix are with `#2291`'s lane evidence.
