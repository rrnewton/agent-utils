---
title: 'threads-view-sent-receipt: a reply sent from the Threads view is drawn once, not also as a Sent receipt at the foot'
status: closed
priority: 2
issue_type: bug
labels:
- vibe-talk
created_at: 2026-10-24T11:39:43.215581801+00:00
updated_at: 2026-10-24T13:02:35.816779354+00:00
closed_at: 2026-10-24T13:02:35.816779214+00:00
---

# Description

[opus 5.5] The owner (rescued Slack message 1792760635, 2026-10-23, Threads view on a desk): "just now I sent a message from the Thread List view. After it sent there was still a card on the bottom AND it rendered as a card under the thread. The same message is on the screen twice." Reproduced on main 285c0885 in Chromium (Threads view, Unpack all, Reply on a thread reply, Send): the reply drawn as row 301 under its thread, and the outbox's 'You · Reply … Sent.' row still at the foot six seconds later. renderOutgoingMessages counts a sent part as drawn only when it is a row of #discord-log; the Threads view draws it among the cards (#side-cards). Not covered by #448 send-dedupe or #467 inline-send-in-place, whose walks reply in All and in a thread.

# Acceptance Criteria

A sent reply whose row is drawn among the Threads view's cards (an unpacked thread's rows or the main rows) leaves no receipt at the foot, as one drawn in a channel list does; a reply still sending keeps its receipt. Page test; the Chromium walk above shows one copy.
