---
title: 'composer-grow-box: on desktop, a checkbox that lets the message box grow with its text'
status: closed
priority: 2
issue_type: feature
labels:
- vibe-talk
created_at: 2026-10-15T19:34:17.418589123+00:00
updated_at: 2026-10-18T20:33:09.774432711+00:00
closed_at: 2026-10-18T20:33:09.774432602+00:00
---

# Description

[opus 5.5] Owner, 2026-10-15: on desktop, pasting a long message leaves the message box tiny, while the space left of Send is empty. Put a small checkbox (for example 'Grow') in that space; when checked, the box grows with typed or pasted text (CSS field-sizing: content where the browser supports it, otherwise resize to scrollHeight on input), up to about 60% of the viewport height, with inner scrolling beyond so Send stays visible. Remember the choice. Phone layout unchanged. Page tests and Chromium checks at 1280x800 and 1920x1080 with a long paste.

# Notes

[opus 5.5] Delivered on branch vt/search-shortcut-grow in "vibe-talk: let the desktop message box grow with its text, behind a Grow checkbox beside Send" and "vibe-talk: check the growing message box in Chromium at 1280x800 and 1920x1080, and the phone unchanged", kept clear of the call's recap line from #331 resume-on-websocket by "vibe-talk: on a desk the call's recap line keeps its own row, so Send never comes up beside it", kept at the foot when Grow is ticked after a paste by "vibe-talk: tick Grow after a paste at the foot of the list, and Send stays on screen as the box grows", with the checkbox kept under the pointer when it is ticked a little above the foot by "vibe-talk: tick Grow a little above the foot, and the checkbox stays under the pointer with Send on screen", and there by a click that focuses no checkbox, as Safari and Firefox on a Mac make it, by "vibe-talk: a click on Grow that focuses nothing, as in Safari and Firefox on a Mac, keeps the checkbox under the pointer too". A Grow checkbox left of Send in the call's, the channel's and the right column's box; ticked, the box grows by field-sizing: content (or the page's measured height where the browser lacks it) up to 60vh and scrolls inside, Send kept on screen; off by default, one stored choice (vibe-talk.voice.composer-grow), desktop only, hidden under a send card. Checked in Chromium at 1280x800, 1920x1080, 1280x600, two columns, a reload with a long draft, the call's box under its recap line, typing at the foot, Grow ticked by a click or Space after a paste at the foot, 30 or 40px above it and from up the list, by a click that focuses nothing (emulated in Chromium), with field-sizing turned off, and a 393x852 phone unchanged.

Unverified here: the fallback in a real Firefox or Safari (only Chromium with field-sizing turned off was measured), a real Mac's click that focuses no checkbox (only Chromium told to keep the focus off it was measured), and the resting height in a browser without the CSS lh unit (it falls back to the 5rem minimum, a few pixels shorter than three rows). Not in this change, as decided: the reply box (Reply screen and a reply docked in a column), which has Cancel beside Send and its own cap; a follow-up if wanted. No Settings control mirrors the choice yet.
