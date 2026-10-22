---
title: 'threads-view-main-messages: the main channel''s messages and box in the Threads view, chips on the glass''s row'
status: closed
priority: 1
issue_type: feature
labels:
- vibe-talk
created_at: 2026-10-22T07:27:40.319329338+00:00
updated_at: 2026-10-22T07:53:27.691001104+00:00
closed_at: 2026-10-22T07:53:27.691000994+00:00
---

# Description

[opus 5.5] The owner, 2026-10-21: "In Cmd-3 thread list view... we currently see all the messages with threads created off them... But we don't see the main channel messages that are not yet threads, and we cannot send messages from this view to the main channel ... In the top we have buttons for Unread Only and Unpack Only... these are weirdly BELOW the Search icon when they should fit on the same row. And we have room for one more button -- a toggle to show Main msgs. When that is on we should see messages to the main channel that are not (yet) threads and can make them threads from there ... if we're already in thread list cmd-3 view I would prefer this just pop open our inline reply under-neath the current message without disrupting the whole view. Further when we have Main messages turned on, we should also have the Reply box (to main channel) as our implicit last card in our list ... I don't want to render multiple text entry boxes so if we click reply somewhere else the text entry box 'jumps' to that point in the stream rather than there being two."

# Design

ai_docs/transient/2026-10-22-threads-view-main-messages.md

# Acceptance Criteria

Threads view: Unread only and Unpack all on the search glass's row; a third chip Main messages (key, ? help, kept per device) that adds the main channel's messages that are not threads yet, in the list's order; Thread or Reply on one opens the #410 inline reply under it, starting a thread, not the thread view; with it on, the main channel's box is the last card; exactly one editor on screen (a reply anywhere moves it, closing returns it to the foot). Shared row, bridge and editor code; page tests failing before with negative controls; Chromium at 1280x800 and 393x852.
