---
title: 'search-date-hint-beside-empty-pins: the date syntax hint stands under the search bar beside "No pinned messages", reading like a date filter hiding pins'
status: in_progress
priority: 3
issue_type: bug
assignee: cl-datehint
labels:
- vibe-talk
- search
- owner-ui
created_at: 2026-11-28T13:38:00-05:00
updated_at: 2026-11-28T14:05:00-05:00
---

# Description

Found by the advisory design review of `#2483` labels-redraw-drops-focus (its after-Enter stills, phone and desk), outside that change. With the search bar open and Pinned in force, the `#2425` date hint stands just under the bar ("after:2026-11-21 · after:7d · before:2026-11-28", today's real example of the date syntax), and directly under it the empty pinned view says "No pinned messages in this channel yet — pin one from its ⋯ menu." Read together, the line looks like a date window in force that might be hiding pins, though no date is set: `searchdates.ts` shows the hint whenever nothing is typed in the search field, and choosing Pinned types nothing. The two "after" dates also name the same day in different forms, which reads as two filters.

# Design

- Show the syntax hint only where it helps: while the search field has the focus and is empty (the reader is about to type), not while another filter (Pinned, a label) stands with an empty field; or word it as an example ("e.g. after:7d") so it cannot read as a filter in force.
- Keep the hint floating and taking no room (`#2425`); the calendar button stays the place that names dates actually in force.

# Acceptance Criteria

- A browser walk (fixture server, never the live server) that fails on main and passes with the fix: with Pinned chosen from the Labels drop-down and nothing typed, no date syntax line stands under the bar (or it reads as an example, per the chosen design); typing in the empty field still shows the hint as `#2425` defined it; a real date window still names itself on the calendar button.
- Owner-UI matrix: desk and phone, light and dark, keyboard open and closed, Discuss open, before and after, with BUILD.txt; the advisory design review.

# Notes

- Filed at the coordinator's request (number assigned by the coordinator) from the `#2483` design review; assigned to cl-datehint. Related: `#2425` (the date hint), `#2489` (the Labels pill in the same stills). The stills: the `#2483` evidence (after/*-4d-dropdown-keyboard-enter.png).
- Settled through the coordinator (2026-11-28, cl-datehint plan approved, from the liaison's fix note):
  - The hint reads as a hint, never a chip: muted italic text starting "Try:", one example per form ("Try: after:7d · before:<today>"), with no pill edge, shadow or chip shape (R4, R5, R15). The title still holds the line whole (R6).
  - It shows only while the search field has the focus AND is empty AND no other filter is in force: Labels (Pinned or a label), Links, Whose. A tap on it still opens the Dates panel (the tap must not blur the field first).
  - Phone: `#2425`'s step-aside while the keyboard is up stays. The matrix shows the phone with the keyboard up and with it dismissed.
- Where it lives: `searchdates.ts` `sync()` writes the text and the only switch (`data-typed`, set once text is typed); nothing reads focus or the other filters. `09-to-do-view.css` gives it the bucket chips' pill (panel fill, edge, shadow, centred, where the chips float). `controlbar.ts` hides it with the calendar.
