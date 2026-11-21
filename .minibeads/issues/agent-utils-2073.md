---
title: 'vibe-talk-new-summary-button: a New summary button next to Update'
status: open
priority: 1
issue_type: bug
labels:
- vibe-talk
- owner-ui
created_at: 2026-11-21T13:55:00-05:00
updated_at: 2026-11-21T13:55:00-05:00
---

# Description

The owner: "I just tried to update button for the first time on an existing summary and I really like it. It works well. I would like a little button next to it for quote new Summary. The idea is that maybe I want to cut a fresh summary with a new messages since the last one instead of updating the current summary with the new messages, both are valid options. If I click new summary, of course the old one goes into the timeline as an earlier summary."

# Design

Next to a summary's Update button, a small New summary button that summarises only the messages since the previous summary's end; the previous summary stays in the timeline as an earlier summary, labelled with its range. Update keeps its behaviour.

# Acceptance Criteria

- New summary creates a second summary over the newer range and keeps the first; browser checks with a model stub; screenshots phone and desktop, light and dark.

# Notes

Filed by the coordinator so the issue link resolves on main; the owning lane refines this file as it works.
