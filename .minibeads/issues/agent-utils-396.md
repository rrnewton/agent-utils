---
title: 'thread-color-memory: a thread keeps its colour across a reload and a channel switch'
status: open
priority: 2
issue_type: bug
labels:
- vibe-talk
- threads
- ui
created_at: 2026-10-20T13:59:33.062215704+00:00
updated_at: 2026-10-20T13:59:33.062215704+00:00
---

# Description

[opus 5.5] Owner, 2026-10-20: refreshing the installed app (Cmd-R) brings the channel back with every thread in a different colour. Switching to another channel and back reshuffles them too. A thread's colour is how a reader picks it out of the list, so it should stay the colour it was on this device.

# Design

Client side: localStorage, per provider + channel + thread. Server-side sync across devices is a possible follow-up only if asked.

# Acceptance Criteria

On one device, a thread drawn in a channel comes back in the same colour after a reload, and after switching to another channel and back. A thread first seen after the reload gets a colour well separated from the remembered ones. No thread changes colour because another one arrived. What is remembered is bounded per channel and in total, the least recently seen going first, so a channel's old threads stop crowding the colours new ones can have. A browser that refuses storage (private mode, full quota) still colours threads as before, for the session.
