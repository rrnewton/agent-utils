---
title: 'threads-main-own-center: the owner''s main-channel rows in the Threads view stand centred'
status: open
priority: 1
issue_type: bug
labels:
- vibe-talk
created_at: 2026-10-22T13:54:26.740740906+00:00
updated_at: 2026-10-22T13:54:26.740740906+00:00
---

# Description

[opus 5.5] The owner, 2026-10-22: "so the coding agent vs me main messages display differently... agents messages are indented left, mine are indented right. But a side effect of that is that my messages now look like they are follow ups to the prior thread because they look the same as the prior thread messages except without the colorful connector. I propose in this view we actually CENTER my messages to the main channel, keeping their current width, so their left edge is no longer lined up with the intra-thread messages."

# Design

In the Threads view only, for the #419 threads-view-main-messages rows (data-main-row) of the owner's (data-who="me"), the existing own-row inset rule gains a selector that splits the inset into equal halves either side: the box keeps its width and is centred in the column, its left edge clear of the unpacked and bridged rows of the threads. The agent's main rows and every row in other views keep their rules.

# Acceptance Criteria

Chromium at 1280x800 and 393x852: an own main row's centre is the column's within 2px, its width as before, and its left edge at least 16px from the previous thread's last unpacked row's. Negative controls: an agent's main row and an own row in All unchanged. Before and after screenshots; a page test of the selector.
