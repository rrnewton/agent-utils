---
title: 'paste-never-sends: pasting into a message box on Android sends at once'
status: closed
priority: 0
issue_type: bug
labels:
- vibe-talk
created_at: 2026-10-26T22:26:55.321375517+00:00
updated_at: 2026-10-26T22:27:09.257454811+00:00
closed_at: 2026-10-26T22:27:09.257454661+00:00
---

# Description

[opus 5.5] Owner, 2026-10-26 14:34: "I pasted this message from another messaging app. And as soon as I press paste on Android, it is sending it immediately without giving me a chance to edit. This was a partial message that I had intended to finish here in vibe talk... Just pasting something should never immediately send it." Three identical sends in a row, multi-line text copied from another app.

Cause: a keyboard's clipboard paste hands Chrome its text without a composition, and Chrome on Android turns each committed newline into a trusted Enter keydown, which onComposerKey took as the reader's Enter while "Enter to send" is on. Wanted: a paste (any *FromPaste input, a drop, any multi-character insertion outside a composition) only ever inserts text; only a real Enter or the Send button sends; in the call box, the channel/thread box and the reply box.
