---
title: 'vibe-talk-discuss-resummarize-stale-terms-and-dated-labels: stale terms stay current during re-summarize; earlier summaries need dates'
status: open
priority: 0
issue_type: bug
assignee: mu-discusstweak
labels:
- vibe-talk
created_at: 2026-11-17T17:36:23.000000000+00:00
updated_at: 2026-11-17T17:36:23.000000000+00:00
---

# Description

[watermelon-full-preview] Owner request (owner thread 135, 2026-11-17), verbatim: "TODO: discuss summary tweak / I just pressed "Summarize" to replace a stale summary with a new one... while it was updating and said "Reading..." the stale "Terms I didn't understand" stayed visible/constant. / Also the "Earlier summary HH:MM" in the timeline should show Date AND time since they are probably very often from previous days."

Two defects in the Discuss panel (`vibe-talk/web/src/view/catchup/panel.ts`):

1. `fetchFor` (Summarize) clears the answer, footer and stamp and says "Reading…", but never touches `#discuss-terms`: the previous answer's "Terms I didn't understand" list stays presented as current until the new answer lands. The staleness banner over the saved summary has the same problem during the wait. The incremental Update ("Updating…") likewise leaves the previous answer's terms looking current.
2. Timeline "Earlier summary" labels and the "Saved summary" status line use the panel-local `hhmm` (time of day only), so a summary from a previous day is indistinguishable from one made minutes ago.

# Design

While a re-summarize or Update is in flight, mark the previous answer's per-answer blocks as stale instead of current: the terms block is dimmed and its heading gains a "from the previous summary" note, and the staleness banner is hidden for the wait. A failed request restores the previous terms exactly as they stood; closing or changing scope mid-request leaves the saved record to redraw them on reopen. The same marking covers any other per-answer block that would look current (the answer, footer and stamp are already cleared/hidden by the existing code and stay so).

For the labels, follow the message rows' rule (`#729`, `messageTimeLabel`): today's summary keeps the time alone; one to six days back adds the weekday; anything older names the month and day (with the year when it is not this year), using the fixed English month/weekday names the day formatter (`dayLabel`) already keeps. The time itself switches from the panel's 24-hour `hhmm` to the locale time the rows show ("3:42 PM"), so the label reads "Earlier summary · Nov 16, 3:42 PM · N unread". The bound's `since` input keeps `hhmm` (an `<input type="time">` value must stay HH:MM).

# Acceptance Criteria

- While Discuss re-summarizes ("Reading…") or updates ("Updating…"), the previous "Terms I didn't understand" list is visibly marked as from the previous summary (dimmed, with that note) and the staleness banner does not present as current; the new answer's terms arrive unmarked.
- If the request fails, the previous terms are restored unmarked; a mid-request close/scope change leaves the saved record to redraw them.
- "Earlier summary" timeline labels show date and time for summaries not from today ("Earlier summary Nov 16, 3:42 PM"), weekday plus time within the last six days, and time alone for today's; the "Saved summary" line follows the same rule.
- Focused panel tests cover the stale marking, its restore on failure, and the dated labels (today, recent, older, other-year); the existing catchup suite stays green.
- Screenshot matrix (phone and desktop, light and dark, keyboard open/closed, Discuss docked, plus a mid-re-summarize shot) captured on a local test instance and reviewed before announcement.

# Notes

Owner UI fast-track (owner thread 135). Page-only change; path-selected validation gate.
