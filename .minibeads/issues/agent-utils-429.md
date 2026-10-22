---
title: 'keyboard-hides-bar: hide the channel view''s Read and Pace tile while the on-screen keyboard is up'
status: open
priority: 2
issue_type: feature
labels:
- vibe-talk
created_at: 2026-10-22T15:04:51.364563144+00:00
updated_at: 2026-10-22T15:04:51.364563144+00:00
---

# Description

The owner, 2026-10-22: writing a message in the channel view on a phone, the open keyboard plus the big blue Read and Pace tile left too little screen to read the message being answered, and nobody writing a message is also being read to. Detect the on-screen keyboard (a text field focused AND the visual viewport well short of the tallest it has been at this width, or navigator.virtualKeyboard reporting a keyboard), hide the channel view's bottom tile (Show/Fold/Hide read, Pace, Read) while it is up, and give its room to the list. A desk or a phone with a hardware keyboard focuses the same field with no viewport shrink, so it keeps the tile. Read-aloud already playing keeps playing; a slim Stop stays reachable. Restore on keyboard close or blur, with the reader's place kept.
