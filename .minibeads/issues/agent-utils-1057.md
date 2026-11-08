---
title: 'newest-count-chip: the Newest chip counts the messages below the reading position'
status: open
priority: 1
issue_type: feature
assignee: mu-refactor
created_at: 2026-11-08T12:55:00.000000000+00:00
updated_at: 2026-11-08T12:55:00.000000000+00:00
---

# Description

The owner (2026-11-08) uses the reading bookmark to jump back into the All messages feed. In a long history the scroll bar gives only a vague sense of how far back they are. The floating Newest chip is in a stable place while scrolling, so it should carry a count: "Newest (N)", where N is the number of messages below the reading position. That shows at a glance whether they are 2, 10 or 20 messages back, and how much reading is left to catch up.

# Design

- The chip reads "Newest (N)", N = the rows below the reading position (the last fully visible row) in the current feed/view.
- Count exactly the rows the current view shows: in All view a reply drawn as its own row counts; replies folded under a parent do not ("how much reading is left" = what you would scroll past).
- Count from the feed's data model, not by walking the DOM, updated on scroll and rAF-throttled.
- A partly loaded feed counts what the view holds; when newer pages are not loaded, show "N+".
- Keep the existing "N new" wording for unseen arrivals (view/channel/folding.ts builds the label) and combine the two as "3 new (N)"; the commit states the choice.
- Always on, no setting.
- Code lives in view/channel/folding.ts and scrollkeep.ts; no voice.js change expected. Stay clear of #624 move 4 (reply glue) and #1052 post-to-main's reserved ranges.

# Acceptance Criteria

- A page test: the count is right while scrolling (top, middle, bottom where the chip hides), with a folded reply not counted and a reply row in All view counted.
- A page test for "N+" when newer pages are not loaded, and for the combined "3 new (N)" label.
- A real-browser check of the chip's count after scrolling back in a long feed.
- No measurable scroll jank: the count runs at most once per animation frame.

# Notes

Owner request, chat reply 225 (2026-11-08), relayed by the lead with its spec. Ships to the next vibe-talk deploy slot once landed.
