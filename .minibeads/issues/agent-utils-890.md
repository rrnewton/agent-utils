---
title: 'voice-page-linked-highlight: a deep-linked message keeps a prominent highlight until dismissed'
status: open
priority: 0
issue_type: feature
assignee: cl-refactor
labels:
- vibe-talk
created_at: 2026-11-05T15:24:27.277072395+00:00
updated_at: 2026-11-05T15:24:27.277072395+00:00
---

# Description

[opus 5.5] OWNER REQUEST (2026-11-05, 10:17 AM ET). The owner's words, verbatim: "when I load the page, it highlights the linked message... but as soon as I move the mouse I lose it. I think that when we specifically load a page highlighting a message that it should have a PROMINENT highlight that is persistent."

It is an owner-approved exception between #624 voice-page-modularity batches.

# Design

- When the page opens with a deep link (`m=`), the linked message gets a prominent, persistent highlight: an accent border, a tinted background, and a small "Linked message" marker.
- The highlight is a separate state from hover and selection. Mouse moves, keyboard selection, scrolling and re-renders do not clear it.
- It clears only on an explicit dismiss (Esc when nothing else consumes it, or the marker's ×), or when another link is opened, which moves it.
- It scrolls into view on load, and again after a re-render of the list.
- Channel and thread views, on phone and desktop.

# Acceptance Criteria

- Tests:
  - the highlight survives a mouse move, a selection change and a list re-render;
  - Esc and × clear it;
  - opening another link moves it;
  - it scrolls into view on load.
- Gate, land, and send the SHA to the lead for an immediate deploy.

# Notes

Relayed by the lead on 2026-11-05 (owner reply 162). Assigned to cl-refactor after #889. Durable host-independent link formats are a separate, undecided owner topic; no link format change here.
