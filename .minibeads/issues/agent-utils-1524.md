---
title: 'discuss-bound-hours-control: an editable hours end on the Discuss bound'
status: closed
priority: 1
issue_type: task
labels:
- vibe-talk
- discuss
- ui
created_at: 2026-11-14T15:44:00.000000000+00:00
updated_at: 2026-11-14T18:30:00+00:00
---

# Description

[watermelon-full-preview] Owner UI request 053. The Discuss panel's bound line
beside Summarize reads "last [N] unread since [HH:MM]". The owner wants an
editable whole-hours control in the line — "[N] unread, [K] hours since
[HH:MM]" — so the reach in hours is both visible and quick to change from the
keyboard arrows or a phone number keypad. When the line cannot fit on one
row, the whole "since HH:MM" part drops; the hours never do.

# Design

- One bound, three linked ends over a single start instant in
  `vibe-talk/web/src/view/catchup/panel.ts`: editing K sets the start to
  now - K hours and restates N (unread at/after that start) and HH:MM;
  editing N or HH:MM restates K. K is whole hours (min 1, step 1), rounded
  up from the start to now (`hoursSinceStart`), so a small clock move never
  jitters it. A draw guard keeps programmatic restatement from feeding back
  into the input handlers. K reaching past the oldest unread clamps to all
  unread (start = the oldest unread). Now is injected
  (`DiscussPanelDependencies.now`), defaulting to the wall clock.
- Markup stays inside the `.discuss-bound` span in `voice.html`: a
  `discuss-hours` number input with the siblings' disabled, aria-label and
  password-manager-ignore attributes, and the "since HH:MM" words wrapped in
  `.discuss-since-part`.
- Fit in `22-discuss.css`: `.discuss-head` is an inline-size container and a
  container query hides `.discuss-since-part` whole when the head cannot
  hold the line, on phone and on a dragged-narrow desk panel alike.

# Acceptance Criteria

- The line reads "[N] unread, [K] hours since [HH:MM]" beside Summarize,
  one row at desktop and phone widths; on the narrow case only the whole
  "since HH:MM" part is gone and the hours remain.
- Editing any of N, K or HH:MM restates the other two consistently, with
  no feedback loops; K clamped past the oldest unread covers all unread;
  the hours control shares the others' pending / nothing-unread disabling.
- Unit tests cover the K<->N<->since linking with an injected now, and the
  browser walk covers the one-row layout, the narrow drop and a K edit at
  phone and desktop widths.

# Notes

[watermelon-full-preview] Number assigned by coordb. Sibling owner-UI lanes
on the same panel: #1525 discuss fold line, #1526 mobile Discuss dock,
#1527 mobile Summarize bottom bar; whoever lands second rebases and
re-gates. `voice.js` and `web/src/app/main.js` are untouched (#624).

## Resolution

Landed as a5d56431 on main and deployed to the devbig014 Slack instance, live 11:53 AM ET 2026-11-14 (release a5d56431); the 15-minute post-deploy watch passed. Closed by the devbig014 coordinator.
