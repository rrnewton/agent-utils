---
title: 'vibe-talk-transcript-view-reload-and-scroll: the agent transcript view survives a reload and stops jumping at the ends'
status: open
priority: 1
issue_type: bug
labels:
- vibe-talk
- owner-ui
created_at: 2026-11-23T06:35:00-05:00
updated_at: 2026-11-23T06:35:00-05:00
---

# Description

Two defects in the agent transcript view: a page reload does not return to the transcript view (and position), and the view scroll-jumps when the reader goes to or tries to stay at the top or bottom of a transcript.

# Design

- Persist the open transcript view (which transcript, and the reading position anchored to a message) the same way other views are restored, and restore it on reload.
- Find the cause of the jumping at the ends (for example growth or windowing that moves the anchor, or a programmatic scroll fighting the reader) and keep the reader's chosen position: at the bottom it stays pinned to new content; at the top it stays put.

# Acceptance Criteria

- Page tests: reload restores the transcript view and position; holding at the top and at the bottom does not move the view while content loads.
- Shot matrix (phone and desktop, light and dark) with before and after of the reload and of the end behaviour.

# Notes

Owner UI request (fast-track); filed by the coordinator.
