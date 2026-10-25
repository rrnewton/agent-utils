---
title: 'cmd-0-voice-view: Ctrl/Cmd+0 goes to the voice (call) view, as Ctrl/Cmd+1 to 4 go to theirs'
status: closed
priority: 2
issue_type: feature
labels:
- vibe-talk
created_at: 2026-10-25T21:41:48.041719809+00:00
updated_at: 2026-10-25T23:26:35.887370260+00:00
closed_at: 2026-10-25T23:26:35.887370030+00:00
---

# Description

[muse watermelon] Owner, 2026-10-25: "Cmd-0 go to voice view. We have these great constant time shortcuts to jump to views Cmd-1 thru Cmd-4.. but what about voice channel? Make that Cmd-0." Add Ctrl/Cmd+0 to jump to the voice (call) view with the same rules as Ctrl/Cmd+1 to 4 (#281 view-keys): from any of the channel views, from a message box or the search, from Settings and Help opened over the main screen; a held key's repeats taken but choosing nothing; where no view can be chosen (sign-in) the chord stays the browser's. The browser's own Ctrl/Cmd+0 (zoom reset) is kept from it only when the page handles the chord, exactly as 1 to 4 keep the tab switch. The Help keyboard list, the Help prose and the README say the new chord.

# Acceptance Criteria

Ctrl/Cmd+0 from Main, All, the thread list and two columns, from a message box and the search, and from Settings/Help over the main screen, shows the voice view and takes the chord from the browser; on the sign-in screen and with Shift (the collapsed-lines bracket) it does not. Already on the voice view it is taken and stays. Ctrl/Cmd+1 from the voice view still returns to the channel. The fake-DOM suite pins the key handling; Help and README updated.
