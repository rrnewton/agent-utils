---
title: 'vibe-talk-server-hits-collapsed-line: server hits collapse to one line so typing filters instantly'
status: closed
priority: 1
issue_type: task
labels:
- vibe-talk
- search
created_at: 2026-11-17T03:53:00.000000000+00:00
updated_at: 2026-11-17T03:53:00.000000000+00:00
---

# Description

[watermelon-full-preview] Owner, 2026-11-16 (FAST-TRACK, owner rule 286), of the server-snippet section in the search view (#605-#608 server-side-search): "I HATE this section. It is disruptive and makes the page slow. It would be maybe ok if we were pinned to the bottom and it was mostly off screen. But I have LOST the ability to start typing in search and quickly filter the cards on the screen. IT's much worse. I would want this current section to be replaced with just "39 more hits on the server" which you can CLICK to expand into this snippet view and load it on demand. I want my filtering for the current screen and cards rendered there to remain FAST." And: "have more sign posting contextualizing what they are in separators between the two section".

Today the section (#server-search in voice.html: scope line, All-channels checkbox, ol#server-hits, More / Search further back; rendering in olderwalks.ts) stands at the head of the list, and every pass of the on-screen filter (searchrows.ts applySearch) also schedules the server search, whose owner (core/search/server.ts) re-renders that section synchronously on every keystroke: the snippet DOM is rebuilt and the list reflows while the reader is still typing.

# Design

- Typing filters the loaded cards only. No server request, no server-section re-render, and no layout shift may happen while typing: the server search stays debounced until the typing settles (the existing SERVER_SEARCH_DELAY_MS rest), and its rendering is deferred with it, off the critical path. A repeated schedule for the same search is a no-op.
- The snippet list is replaced by ONE tappable line, "N more hits on the server" (N from the server answer's fresh hits, "N+" while a further page remains, "Search the server" until an answer is known). Snippet nodes are built and drawn only when the line is tapped; tapping again collapses them. All channels / More / Search further back keep working inside the expanded view.
- Labelled separators: "On this screen" above the loaded cards' matches, and "On the server, not loaded here" above the server line or expanded snippets, plus "including other channels" while All channels is ticked. Updating them costs the fast filter nothing (they change on search open/close and on the deferred server render, never per keystroke).
- Placement: the server section moves to the foot of the scrolling list, below the newest loaded message, so it sits mostly off screen under the #1734 search-entry landing (which takes the reader to the newest hits); a fixed bottom bar is rejected because it would overlay the composer and the keyboard-shrunken viewport and stay disruptive. The collapsed line is one row tall there, so even its deferred appearance cannot move the cards above it.
- #1734's search-entry scroll default keeps working unchanged.

# Acceptance Criteria

- Typing filters the on-screen cards instantly: no server request fires and the server section does not re-render or shift layout while typing; the server search runs once, after the typing settles.
- The collapsed line shows the server hit count and expands on tap into the snippet view (scope line, All channels, hits, More, Search further back), and collapses again on a second tap.
- The two separators read "On this screen" and "On the server, not loaded here" ("including other channels" added while All channels is ticked).
- Typing latency (per-keystroke filter time) is measured before and after on a fixture with a few hundred cards, phone and desktop, and reported.
- Page tests at phone (412x915) and desktop (1280x800) sizes cover the exact defect, in light and dark theme, with the keyboard open (visualViewport shrink) where layout is involved, and the Discuss half-pane open in at least the phone shots.
- Screenshot matrix per the standing acceptance (light/dark x keyboard open/closed x phone/desktop, Discuss half-pane open in at least the phone shots, before and after the targeted state), reviewed image by image.

# Notes

## Resolution

[watermelon-full-preview] Landed on main as c5dc7a14 (gated tree a9d469d4, replayed over 6e80ecfb). Typing runs only the on-screen filter: the same search scheduled again is a no-op and a new search is neither asked nor drawn until the typing rests. The server section stands at the foot of the list as one tappable line ("50+ more hits on the server"; "N+" while a further page remains, "Search the server" until an answer is known) under "On the server, not loaded here" ("including other channels" added while All channels is ticked), with "On this screen" over the loaded hits; a tap draws the scope line, All channels, the snippets, More and Search further back, and a second tap collapses them. Placement: the foot of the scrolling list, below the newest loaded message, where the #1734 landing leaves it mostly off screen; a fixed bottom bar was rejected because it would overlay the composer and the keyboard-shrunken viewport. Measured on a 300-message fixture (Chromium, synchronous input dispatch): per-keystroke filter median 26.6 -> 21.1 ms phone (412x915), 27.7 -> 26.2 ms desk (1280x800) — the local filter was already the cost and is unchanged in kind; the defect numbers are the answer's arrival: the watched hit row moved 5089.5 px before, 0.0 px after, and the section went from 456 DOM nodes (50 snippets drawn unasked) to 10 (the line alone). Tests: tests/server_hits_browser.py (8/8: light/dark x keyboard x phone/desk, Discuss half-pane open on phone), the #605-#608 fake-DOM stage and tests/search_unmatched_browser.py updated to the tap, search_scroll_browser (#1734) green. Screenshot matrix in /tmp/mu-serverhits-shots (before/ 16, after/ 49, every image inspected; the Discuss panel's 404 line in the shots is the fake API not serving its summary route, present before and after alike).
