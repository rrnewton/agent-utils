---
title: 'vibe-talk-summarize-inputs-free-typing: typing in the Summarize count/hours fields is clamped and rewritten'
status: closed
priority: 0
issue_type: bug
labels:
- vibe-talk
created_at: 2026-11-17T12:00:00.000000000+00:00
updated_at: 2026-11-17T12:00:00.000000000+00:00
---

# Description

[watermelon-full-preview] Owner report (thread 156): typing numbers in
the Summarize view is messed up. The hours field still caps at 99, the
up/down arrows move from 1-24 hours, but typing "1" enters "2". Hours
cannot be typed freely. The message-count field has the same problem.

The two inputs are the Discuss panel's bound row ("[N] unread,
[H] hours since HH:MM") in `vibe-talk/web/src/view/catchup/panel.ts`,
backed by `boundForCount`, `boundForHours`, `countForHours` and
`hoursSinceStart`, with markup in `vibe-talk/web/voice.html`
(`#discuss-count`, `#discuss-hours`, both `type="number" min="1"
step="1"`).

Cause of the 1->2 jump: not the `min` attribute alone. Every `input`
event immediately recomputes the bound and `drawBound()` rewrites
both fields from the recomputed state. `boundForHours` restates the
typed hours as `hoursSinceStart(oldest, now)` (whole hours, rounded
up) whenever the typed window reaches the oldest unread, and forces a
minimum count of 1 (the newest unread alone) when no unread falls
inside the typed window. So typing "1" with the oldest unread 1.2
hours ago posts back "2"; typing a count clamps to the held unread
count and the hours restate rounded up. The dependent-value feedback
loop (typed value -> recompute -> rewrite the field being typed in)
is the defect; the `min="1" step="1"` attributes and the whole-hour
truncation in `boundForHours`/`countForHours` compound it.

# Design

- While typing, neither field is clamped, snapped or rewritten. The
  field being edited keeps the owner's raw text and caret; only the
  other (dependent) field updates. Commit on `input` with a debounce
  or on blur/change, without fighting the caret.
- Typing HOURS sets the time window (`now - H hours`, fractional
  hours allowed); the message count follows from it. Any positive
  hour value is valid even when no messages fall in range (count 0).
- Typing MESSAGES sets the count; hours follows as a dependent value
  shown floored to one decimal place (6.81 -> 6.8), never rounded up.
- Arrow keys still step the field being edited.
- `boundForHours` keeps the typed hours instead of restating them,
  allows a zero count for an empty range, and `countForHours` uses
  the fractional hours as typed.

# Acceptance Criteria

- Typing "1" in hours gives 1; typing "150" gives 150 (no 99 cap).
- Typing a message count shows hours floored to one decimal.
- Hours covering no messages are accepted (count follows as 0).
- Arrow-key stepping still works on both fields.
- Page tests cover the defect at phone and desktop sizes.

# Notes

Owner-priority, deployed on landing. Screenshot matrix per the
standing owner-UI acceptance (light/dark, keyboard open/closed,
phone/desktop, Discuss half-pane open on phone, before/after of the
targeted state) on a local test instance.

## Resolution

Landed as 86e36f72 on main. While typing, the edited field is never
clamped, snapped or rewritten; typing hours sets the window exactly
(fractions allowed, zero messages in range accepted), typing a count
sets the count and the dependent hours show floored to one decimal.
Unit tests 59/59, typecheck clean, discuss browser walk passed at
phone and desktop in both themes, gate passed.
