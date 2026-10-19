---
title: 'hide-empty-messages: a Settings switch, on by default, that leaves empty messages out everywhere'
status: open
priority: 2
issue_type: feature
labels:
- vibe-talk
created_at: 2026-10-19T22:07:19.914469516+00:00
updated_at: 2026-10-19T22:07:19.914469516+00:00
---

# Description

[opus 5.5] The owner, 2026-10-19: "in that screenshot you can see that there is a BLANK message from the coding agent... along with auto-marking read on some trivial messages I want to have a checked-by-default box in that same settings area that completely filters out empty messages". A row with a time, Reply and Done over no body tells the reader nothing and still asks to be dealt with.

# Acceptance Criteria

Settings > Messages has an Empty messages switch beside Mark-read automatically, on by default and kept in this browser as the neighbouring switches are. While it is on, a message with no visible content - no text but white space, no reaction, and not the root of a thread anyone has answered - draws no row in any view, is not counted as unread, is never read aloud or relayed into a call, and draws no row when it arrives live. A file posted with no words is not empty, nor is a message with only a reaction. Turning the switch off draws every empty message again without a read. Page tests and a Chromium walk with negative controls, failing before the change.
