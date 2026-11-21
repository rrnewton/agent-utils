---
title: 'vibe-talk-documents-react: React and discussion jumps in the Documents reader'
status: closed
priority: 1
issue_type: feature
labels:
- vibe-talk
- reader
- documents
- ui
assignee: mu-readstart
created_at: 2026-11-21T00:22:47-05:00
updated_at: 2026-11-21T00:22:47-05:00
---

# Description

[watermelon-full-preview] Owner 276 (verbatim via liaison, 2026-11-20): "As we do the reader and fix the highlights for the document view, I would also like to add an advanced feature. Once we get through that go ahead and automatically add this afterwards. Basically, I want the same react button to work in document view in addition to our normal reaction to messages. This is a bit tricky though. I think we could have a bit of a configurable policy and experiment with it. Option one is we react in a thread reply replying from the most recent message that has linked this document. The heuristic is whatever thread you asked for the document on or mention the document. That's probably where you want follow-up discussions. Option two which would not be the default would be to create a single top-level discussion thread in the main channel of whichever channel most recently mentioned the document. If the threat already exists with a standard schema and format I.e a root message that says discussion of document blank as its message. If it already exists then we use the existing one and continue replying in that thread. Either way I want a button on the bottom toolbar of the document view. A button that jumps to last mention of the document. And a button that jumps to discussion of the document. If there has been no discussion via react then that second button just jumps to last mention as well. No I take that back. It can just fizzle with a little message that says there's been no discussion yet and you can use the other button if you want to jump to last mention."

Queued after #2003 vibe-talk-read-starts-on-screen and #2004 vibe-talk-read-highlight-missing land (they have: 47454e8a). Design note first, sent to the coordinator before any code (see Notes).

# Design

Design note sent to the coordinator before code; the settled design is recorded here when approved. Sketch (from the note): the strip's React works while a document read stands (today it waits disabled, as a document holds no sentence), holding the lit block's words; Send posts as the owner through the one reply route into the policy's thread. Policy is a synced choice, default A: (A) the thread of the most recent message that mentioned the document (`reply_to` that message), or (B) one top-level thread in that mention's channel whose root says exactly "Discussion of document <name>", found first and created only by a React send, never by a jump. Two reader-toolbar buttons, Jump to last mention and Jump to discussion; with no discussion yet the second shows a short notice naming the first button and does not jump. React posts as the owner, so tests run on fixtures and test instances only, never the live server and never a real channel.

# Acceptance Criteria

- The design note is approved by the coordinator before any code lands.
- React in the Documents reader posts the reader's dictated words as the owner into the configured policy's thread, quoting the block that was being read; policy A is the default and policy B reuses an existing standard-format root.
- Jump to last mention lands on the most recent mentioning message; Jump to discussion lands on the discussion thread, or shows the no-discussion notice and does not jump where none exists.
- All React tests run on fixtures/test instances only.

# Notes

[watermelon-full-preview] mu-readstart. Owner-priority, queued after #2003/#2004 (landed 47454e8a, closed 366598dc). Design note first; no code until the coordinator approves it.

## Resolution

[watermelon-full-preview] Landed on main as 46541238 (combined tree 2ed82b48 over 09e7256c, coordinator waiver on the gate's single load-timing red; combined tree verified: typecheck, page shards 0-3, and the documents/keep-place/discuss/done-sync/view-switch/resume browser checks). The strip's React works while a document read stands, quoting the lit block and posting as the owner: policy A (default) into the last mention's thread, or policy B into one "Discussion of document <name>" thread, created only by a Send. The reader's toolbar carries Jump to last mention and Jump to discussion, with the no-discussion notice where none stands. Follow-on found in the same shots: #2033 vibe-talk-documents-newest-chip (pre-existing, filed separately).
