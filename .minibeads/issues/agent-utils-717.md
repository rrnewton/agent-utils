---
title: 'vt-unread-recap: recap CLI showing unread window grouped by thread'
status: open
priority: 2
issue_type: feature
created_at: 2026-11-01T00:49:01.619754387+00:00
updated_at: 2026-11-01T00:49:01.619754387+00:00
---

# Description

The owner marks vibe-talk messages Done as reading progress. After 12-24 h away there are too many updates, many reporting transient statuses, and agents asked for a recap cannot tell what the owner has already read.

Add `vibe-talk recap`: a CLI that talks to a running server with the READ token and prints a window of chat history (oldest first, grouped by thread), each message tagged DONE or UNREAD from the per-message Done state (POST /channels/{id}/dismiss and /restore), with the per-channel read mark shown in the header only. Options: --hours N or --since <permalink|message id|time>, optional --channel (default every channel the token sees), --max-chars, --json. Threads started before the window show their start line, hidden-reply count, root, a bounded CONTEXT opening, then only in-window replies. Over budget, shrink DONE messages to one line first; never cut UNREAD. Add a READ-scope server endpoint if existing routes cannot answer. Add a vibe-talk-recap skill that reports by topic without repeating read material unless its status changed later in the window.

Scope: server, CLI and skill only. web/, voice.js and voice.html are frozen (#624 voice-page-modularity).
