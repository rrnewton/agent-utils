---
title: 'settings-prose: no implementation prose on Settings; one-line descriptions become tooltips'
status: closed
priority: 2
issue_type: feature
labels:
- vibe-talk
created_at: 2026-10-18T20:43:26.571058928+00:00
updated_at: 2026-10-18T21:14:53.046766186+00:00
closed_at: 2026-10-18T21:14:53.046766025+00:00
---

# Description

[opus 5.5] Owner, 2026-10-18 ("settings complaints"): remove implementation-detail text from Settings, for example "Live updates are configured for adapter push, and this page is connected to the server stream..."; "Arriving messages during a call" becomes the section title, with only its drop-down; rename "Read automatically" to "Mark-read automatically" (he asked for this before, and the request never reached an agent); one-line descriptions ("Same author, within five seconds...", "Turn it off to see each message's first line...") become the hover tooltip of the (?) and of the subsection title, and clicking the (?) opens the full help; "How messages are said" becomes a short link, "How messages are pre-processed for speech", into Help; rewrite or remove "Car audio: use Agent audio.", which means nothing to him. Every control keeps its id and its storage key. Page tests with negative controls, and screenshots at 393x852, 1280x800 and 1920x1080.
