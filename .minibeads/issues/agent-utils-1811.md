---
title: 'vibe-talk-discuss-thread-scope-and-title: scope Discuss and its Summarize count to the open thread, title names the scope'
status: closed
priority: 1
issue_type: bug
labels:
- vibe-talk
created_at: 2026-11-17T20:01:00-05:00
updated_at: 2026-11-18T03:30:00-05:00
---

# Description

Owner report: zoomed into a thread view, the Discuss "Summarize N unread" row showed a very large unread count — the channel-wide count, not the thread's. The header should read "Discuss: main channel" in the main view and "Discuss: thread <slug>" inside a thread.

Finding: a fresh open inside a thread already scopes (the preview and POST carry the thread id, and the server reads that thread alone). The unscoped path is entering a thread while Discuss stands open bound to the channel: in-channel view changes never called the Discuss sync at all, and where sync did run the scope-binding rule kept the panel bound and labelled global and never re-read the preview, so the unread count, the summarize range and the Summarize POST all stayed channel-wide.

# Design

- Entering a thread with Discuss open re-scopes it to that thread: the binding, title, unread preview and summarize bound all become the thread's; the panel stays open.
- Returning from a thread to the main channel with Discuss open re-scopes it the other way: it re-aims at the main channel (unread count, summarize bound and context become the channel's) and the title updates to "Discuss: main channel". Discuss always scopes to the view on screen; sideways thread-to-thread and channel changes still close behind a history entry, as before.
- The panel title names the scope: "Discuss: main channel" for a channel scope, "Discuss: thread <slug>" for a thread scope, using the thread's filed name the scope already carries.

# Acceptance Criteria

- In a thread view the unread count, summarize range and Discuss context are that thread's messages only, including when Discuss was opened before entering the thread.
- The panel title is "Discuss: main channel" in the main view and "Discuss: thread <slug>" inside a thread.
- Page tests at phone and desktop sizes cover the defect; screenshot matrix captured and inspected.

# Notes

## Resolution

Landed 972439e26241c25a93c0c0bc41b09c766853d56b on github/main. Entering a thread re-scopes an open Discuss to that thread (and returning to main re-scopes back), via the new narrowing-rescopes-to-thread / widening-rescopes-to-main binding rules and syncDiscussView wired into in-channel view renders; the panel title reads "Discuss: main channel" or "Discuss: thread <slug>" and truncates with an ellipsis at narrow widths. Verified: catchup and channel JS suites, typecheck, discuss-browser and discuss-dock-browser solo on the combined tree, and a 12-image screenshot matrix (phone 412x915 and desktop 1280x800, light and dark, keyboard open and closed) inspected by the lane and confirmed by the coordinator. mention-jump-browser passed 3x solo on this tree and 3x on unmodified main; its one gate failure was host-load flake.
