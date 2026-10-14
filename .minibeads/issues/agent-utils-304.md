---
title: 'back-gesture-in-app: the phone''s back gesture closes the open thread or screen instead of leaving the app'
status: closed
priority: 1
issue_type: feature
labels:
- vibe-talk
created_at: 2026-10-14T16:55:08.525568906+00:00
updated_at: 2026-10-14T20:42:16.360120466+00:00
closed_at: 2026-10-14T20:42:16.360120337+00:00
---

# Description

Owner 2026-10-14: on the installed Android PWA, the system back gesture inside a thread exits the app to the home screen; only the on-screen Back button closes the thread. Make the back gesture (and the browser's Back) act inside the app: opening a thread (one column, the right column, the thread list's thread), the thread list (Cmd-3 screen), the reply screen, Settings, Help and similar full screens pushes a history entry; popstate closes the top one exactly as the on-screen Back/Esc does; the on-screen Back and Esc call history.back() when they close a screen that pushed an entry, so the two stay in step; at the top level back leaves the app as usual (no trap); a reload or a deep link never leaves a stale stack. Consider CloseWatcher (Chrome 120+) for menus and dialogs. Must not break #281 view keys, #280 scroll stability, #302 root-is-the-app (/ vs /voice) or desktop browser Back.
