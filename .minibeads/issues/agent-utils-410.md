---
title: 'inline-reply-editor: Reply opens in the stream under the message and its replies, one box at a time'
status: closed
priority: 1
issue_type: feature
labels:
- vibe-talk
created_at: 2026-10-22T02:42:17.865917915+00:00
updated_at: 2026-10-22T05:38:07.447992901+00:00
closed_at: 2026-10-22T05:38:07.447992630+00:00
---

# Description

[opus 5.5] The owner, 2026-10-21: "When I press 'reply' on a message inside a thread in two column view, I see a reply box pop up AT THE BOTTOM. But I STILL see the other 'inline' reply box at the bottom of the thread message series stating 'Reply in this thread...' Why don't we move the 'Reply' box to also be an inline box and NOT show two at once. ... when we press reply we switch into the same kind of bridge/connector view we use elsewhere -- gather the existing replies underneath the message we just pressed Reply on, and then our new Reply interface is just the existing inline-message editor (with Grow button) inserted in the stream. ... This will get rid of the jarring 'jump to a different screen' effect when we currently press reply. ... In the main thread it puts our entry box just below the message we're replying to, which is nice, but it omits previous replies which is not great." Today: in two columns Reply docks a second box at the column's foot while the column's composer stays drawn; in one column Reply is a separate screen that draws the message with only what came before it.

# Design

ai_docs/transient/2026-10-21-inline-reply.md

# Acceptance Criteria

In Main, All, a thread drill-in, both columns and the Threads view with Unpack all: Reply keeps the reader where they are (no screen change); the message's replies are gathered under it with the shared bridge (#215 gathering, #407 unpackBlock for a Main root's thread); the reply box opens directly under them in the stream with Grow, drafts, Send and Esc; later messages stay below. Only one box: the column's composer is hidden while the reply is open and comes back on close or send. After Send the reply lands in place and the reader does not jump. r opens, Esc closes, Ctrl/Cmd+S and c go to the open box; Help updated. Page tests fail before, with negative controls; Chromium walk at 1280x800 two columns and 393x852 replying to an old message mid-stream.
