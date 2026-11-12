---
title: 'discuss-summarize-control-oneline: one line — [Summarize] last [N] unread since [HH:MM picker]'
status: open
priority: 0
issue_type: bug
labels:
- vibe-talk
created_at: 2026-11-12T17:10:00+00:00
updated_at: 2026-11-12T17:10:00+00:00
---

# Description

Owner replies 005/006 (via coordb, trying the live Discuss panel): "the summarize controls kinda worked but present very badly: the HH:MM clock is obscured by its own clock button, not spaced correctly; the `last [56] unread` text entry box is too big for 2-3 digit message counts; the Summarize button is LONG and repeats the same text".

Today (#1346 discuss-summarize-button, landed) the control is split over two rows: the header's #discuss-actions holds a Summarize button whose label repeats the bound ("Summarize last 56 unread since 14:30"), and a separate .discuss-ask row holds "last [count at 4.5rem] unread since [time]". The native time input draws its picker indicator over the HH:MM digits, and the count input is sized for far more than the 2-3 digit counts it holds.

# Design

- One line, in the panel's control row: `[Summarize] last [N] unread since [HH:MM ⏱]`. The Summarize button (#discuss-summarize, id unchanged) says only "Summarize"; the bound's linked ends (#discuss-count, #discuss-since, ids unchanged) sit in the same line and keep their two-way linking (editing N moves the time, editing the time moves N; panel.ts `boundForCount`/`countSince` unchanged).
- The count input is narrowed to a 3-digit width. The time input gives its picker indicator its own space (padding and indicator spacing in 22-discuss.css) so the HH:MM digits are never under the clock button; the indicator remains the time-picker control the owner named.
- `drawBound()` no longer writes the long repeated label; it draws only the two ends and the button's disabled state, as before. No server, contract or cache change: Discuss still reads only the vibe-talk cache (owner rule 414).

# Acceptance Criteria

- With unread ready, the panel shows one line: a short "Summarize" button, "last", a narrow count input (3 digits), "unread since", and an HH:MM time whose digits are not overlapped by its picker button at 390x844, 375x667 and 1280x800.
- The button never repeats the count/time text; N and time stay linked both ways; pressing Summarize posts exactly the chosen bound; while pending every Discuss entry point stays disabled.
- Page tests pin the short label, the linking and the pending guard; the Discuss browser walk asserts the one-line control; the owner-tier gate passes.

# Notes

Owner replies 005/006. Lands with #1383 discuss-header-icons as one owner-tier landing (shared header/control row), agreed with mu-catchup (the four ids #discuss-summarize/#discuss-count/#discuss-since/#discuss-close stay stable) and mu-discussfoot (answer/footer untouched).

## Resolution

[watermelon-full-preview] Delivered in e5fc567c (4:30 PM ET Discuss small-fix batch, owner reply 012; integrator mu-discussfoot, gated 851343ad). The Summarize control is one line — [Summarize] last [N] unread since [HH:MM picker] — with a short button, a 3-digit count input, and the time picker given its own space so the digits are never obscured. Focused tests and the Discuss browser walk passed in the batch gate; screenshots at 390x844/375x667/1280x800 were recorded in the lane handoff.
