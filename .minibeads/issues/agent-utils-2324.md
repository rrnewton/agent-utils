---
title: 'vibe-talk-shots-scene-35-settle: the message-search screenshot waits for the server search to settle before it is taken'
status: open
priority: 2
issue_type: task
assignee: cl-flaketriage
labels:
- vibe-talk
- screenshots
created_at: 2026-11-26T19:30:00-05:00
updated_at: 2026-11-26T19:30:00-05:00
---

# Description

Scene `35-message-search` in `vibe-talk/scripts/screenshots.py` (light laptop and the other matrix profiles) photographs the channel while the server-side search for the quoted phrase is still in flight: the picture shows the "Searching the server..." row instead of the settled result. Its act waits only for a visible match and for the load-older control to stop saying it is searching, which can hold before the server search answers. A screenshot of a transient state is not evidence of what the search shows, and whether it is caught depends on timing.

# Design

Harness-only: after the existing waits, `_act_message_search` waits on a condition, not a time, for the server search to settle: the "Searching the server..." status gone (the server-search row showing its result or its end state). The scene's expectations gain the same condition, so a run that still catches the search in flight fails as not reached instead of capturing it. No page or product code changes.

# Acceptance Criteria

- Scene 35 waits on the server search's settled state (a DOM condition) before capture; no fixed sleep is added.
- The scene's expectations refuse a capture showing the in-flight search status.
- Before and after shots of scene 35 (light laptop at least, plus the matrix profiles it runs on) show the in-flight status before and the settled result after; the desk and phone walks stay green.

# Notes

Evidence of the in-flight capture: a before/after reshoot made by another lane on 2026-11-26 shows "Searching the server..." in both. Number assigned by the coordinator; ordered after `#2311`.
