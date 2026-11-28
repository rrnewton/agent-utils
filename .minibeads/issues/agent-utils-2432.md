---
title: 'vibe-talk-keyboard-shift-e-status-race: keyboard-browser reads the Shift+E status before the server has answered, on a prefix the optimistic words already match'
status: open
priority: 2
issue_type: bug
labels:
- vibe-talk
- tests
created_at: 2026-11-27T21:20:57-05:00
updated_at: 2026-11-27T21:20:57-05:00
---

# Description

[opus 5.5, cl-signpost3] vibe-talk.check.keyboard-browser fails on unrelated gates (27570ce1, 2026-11-27), for example:
"the status line does not say the message was read already in both places: 'Already read here, and collapsed.'"

The race is in the test.
- Shift+E first says the local words at once ("Already read here, and collapsed.", the optimistic Done of #892, present/notices/marks.ts localReadWords).
- It says the server's answer ("Already read here and in Google Chat, and collapsed.") over them once the server answers.
- tests/keyboard_browser.py polls only until the status STARTS WITH "Already read", which the optimistic words match before the server answers. Two frames later it checks the server's request record and the server's sentence.
- The first Shift+E's check ("Marked read here and in Google Chat") is built the same way.

Shown by bisect and repro:
- 5/5 pass on 1bf29fe1 and on 2eebd8ea alike.
- With the fake API answering the server's half (POST .../upstream-read) 400 ms late, both fail the same way.
- "Google Chat" is the fixture's own chat_provider_name, not stale page wording.

# Design

- Wait on the server's half having happened: the fake API has recorded the upstream-read, AND the status equals the server's sentence. A condition with the hang backstop, never a prefix the optimistic words match.
- The same for the first Shift+E's "Marked read here and in Google Chat" check.

# Acceptance Criteria

- With the server's answer held 400 ms on the fake API's server thread, the old check fails and the new one passes.
- A negative control (a page whose server half never changes the status) still fails it.
- Repeated runs pass on a busy host (load1 recorded).

# Notes

Filed at the coordinator's request after the #2375 bisect (issue file only); the fix follows #2428.
