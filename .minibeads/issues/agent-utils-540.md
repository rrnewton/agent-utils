---
title: 'read-aloud-desktop: reading aloud works the same in every view and on desktop'
status: open
priority: 1
issue_type: bug
labels:
- vibe-talk
created_at: 2026-10-26T16:14:55.059361409+00:00
updated_at: 2026-10-26T16:14:55.059361409+00:00
---

# Description

[opus 5.5] The owner, 2026-10-26: on the desktop PWA, Read in the channel view brings up the overlay but no audio in cloud mode; in device mode the thread list (an unpacked child of a thread) says the message was not loaded; in All the paragraph/sentence animation runs too fast with no sound; in the thread list the chunks animate on the timeline but the message itself is not lit. Thread cards should be readable too.

Reproduced in Chromium against the checked-out page:
- Device voice in the thread list (unpacked rows, Main rows among the cards): 'That message is no longer loaded' — the device path looked the row up in #discord-log/#pinned-log only, and the cards' rows live in #side-cards.
- Sentence light: looked up only in the left column's #discord-log, so a read in the thread list or the right column's thread lit no sentence; the block light moved only in #discord-log/#pinned-log.
- Thread cards: a tap in reading mode cycled or highlighted the card; nothing was read.
- Cloud audio silent: the deployed server's logs (read-only) show every 2026-10-26 read-aloud turn ending in ~0.65 s with no speech onset reported by the page (0/60 timing reports carry tap_to_speech_ms; 130/130 did on 2026-10-25). The voice provider returned silence; the page raced through the blocks and archived the message as heard. Reported to the coordinator; the page now stops a read whose part ends without speech, says so, and archives nothing.
