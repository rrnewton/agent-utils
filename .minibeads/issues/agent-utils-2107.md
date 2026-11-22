---
title: 'vibe-talk-shortcut-hover-hints: every button with a keyboard shortcut shows it on hover'
status: open
priority: 2
issue_type: feature
labels:
- vibe-talk
created_at: 2026-11-22T10:10:00-05:00
updated_at: 2026-11-22T10:10:00-05:00
---

# Description

The owner: every button that also has a keyboard shortcut should hint that shortcut on hover on desktop. Also related: a Cmd-2 binding the owner reported as not working.

# Design

Drive hints from the ONE shortcut registry so a hint can never drift from its binding: desktop (fine pointer) hover title or tooltip on each button with a binding, platform-aware (Cmd on Mac, Ctrl elsewhere). While auditing the bindings, check the Cmd-2 report and fix or explain it.

# Acceptance Criteria

- A test asserts every registered binding that has a button shows its hint with the right modifier; the Cmd-2 binding works or the issue explains why; screenshots on desktop light and dark.

# Notes

Filed by the coordinator; the owning lane refines this file.
