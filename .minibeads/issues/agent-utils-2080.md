---
title: 'vibe-talk-summary-link-stays-in-view: a summary''s message link must stay in the current view, not jump to the thread view'
status: open
priority: 1
issue_type: bug
labels:
- vibe-talk
- owner-ui
created_at: 2026-11-21T17:53:00-05:00
updated_at: 2026-11-21T17:53:00-05:00
---

# Description

The owner: "CRAP I'm in the All view and got a big summary open.. I'm trying to click the messages in the generated summary. It is consistently jumping into the THREAD view which jumps AWAY From my index that I am navigating!!! I want to STAY on the discuss window and all view and just jump around the relevant messages. If i WANT I can click into a thread and drill in then press back."

# Design

A message link inside a summary (Discuss open, All view) scrolls to and highlights that message in place in the current view, keeping Discuss open and the summary's own scroll position. It never switches to the thread view. If the message is not loaded in the current view, it is loaded in place. Opening a thread stays an explicit action, and Back returns to where the reader was. Related earlier work: #1392, #1586, #1820, #1911.

# Acceptance Criteria

- From a summary in the All view, every message link keeps the view and Discuss open and lands the message on screen highlighted, including a reply that belongs to a thread and a message not yet loaded.
- Browser checks drive the real page and router (no stubbed navigation), phone and desktop; screenshots light and dark.

# Notes

Filed by the coordinator so the issue link resolves on main; the owning lane refines this file as it works.
