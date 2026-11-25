---
title: 'vibe-talk-wtf-popover-late-answer-reopens: a WTF answer that lands after the popover closed must not reopen it'
status: open
priority: 2
issue_type: bug
labels:
- vibe-talk
- owner-ui
created_at: 2026-11-25T08:41:00-05:00
updated_at: 2026-11-25T08:41:00-05:00
---

# Description

Found while evidencing #2231 vibe-talk-selection-strip-atomic. A press of WTF? opens the glossary popover at once with "Looking this up…" and then awaits the lookup. If the reader closes the popover (its X, Escape, or a tap elsewhere) before the answer arrives, the answer still calls `showPopover` when it lands and opens the popover again, over whatever the reader went on to do. Screenshot scene 97-glossary-orphan-popover raced exactly this: it closed the popover while the lookup was in flight and the late answer reopened it over the shot (the scene now waits for the answer before closing, so it no longer shows the race).

# Design

- The glossary view keeps a lookup generation (or the pending lookup's token): a press starts a new one, and `closePopover` ends it.
- When a lookup answers (found, not found, or failed), it shows the popover only if its generation is still the current one and the popover was not closed meanwhile; otherwise the answer is dropped (a found term may still be saved and linked, without opening anything).
- A second press while a lookup is in flight supersedes the first: only the newest answer shows.

# Acceptance Criteria

- A page test: press WTF?, close the popover before the lookup resolves, then resolve it: the popover stays closed; and a control where the answer arrives while the popover is open still shows it.
- A page test: two presses in a row, the first answer landing last: only the second answer shows.
- Condition-based waits only (a held lookup the test releases), at phone and desktop sizes.

# Notes

Pre-existing in the glossary view since #2156 vibe-talk-wtf-glossary-lookup; not changed by #2231. Filed at the coordinator's request as a side finding of #2231.
