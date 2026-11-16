---
title: 'vibe-talk-quote-reply-dismiss-x: a dismiss X beside the inline quote-reply chip'
status: open
priority: 0
issue_type: task
labels:
- vibe-talk
- owner-ui
created_at: 2026-11-16T14:45:00+00:00
updated_at: 2026-11-16T14:45:00+00:00
---

# Description

[watermelon-full-preview] Owner, :thread:117 at 9:45 AM ET: "Give me a little X next to the inline quote reply button. When I do a selection accidentally, I don't have a good way to clear it. If I tap the message then I collapse a very big message and have to open it up again and scroll."

When a text selection shows the inline quote-reply button (the selection chip, `#726 vt-selection-chip`), show a small X beside it. Tapping X clears the selection and hides the button and the X.

# Design

Reuse the selection chip's styling and placement owner (`drawSelectionChip` in `web/src/view/channel/quoting.ts`): a companion dismiss button (`#selection-chip-dismiss`) shown, hidden and placed with the chip, just beside it. Its press clears the browser selection and lets the held quote go (`holdQuote(null)`), and is stopped before it can reach any card or list handler, so the tap neither toggles the message's collapse/expand nor moves the scroll position, and takes no focus. The same X appears wherever the chip does, desktop included; unobtrusive, with at least a 32px touch target on phones. No other changes.

# Acceptance Criteria

- With text selected in a message, the X stands beside the Quoted reply chip and the two do not overlap.
- Tapping X empties the selection; the chip and the X are gone.
- The message card stays as it was (an expanded long message stays expanded) and the list's scroll position does not move.
- A page test at a phone viewport (about 412x915, device scale 2.6) covers the above with condition-based waits only.
- Acceptance screenshots on a local test instance at phone width: selection with chip and X shown, and after the tap.
