---
title: 'vibe-talk-search-view-scroll-default: entering search keeps the reader in place or goes to the newest hits'
status: open
priority: 1
issue_type: task
labels:
- vibe-talk
- search
created_at: 2026-11-16T20:30:00.000000000+00:00
updated_at: 2026-11-16T20:30:00.000000000+00:00
---

# Description

Owner, 2026-11-16 15:19 ET: "When I enter search view, what effect does that have on my scrolling? I would like it by default to stay where I am (if any message on screen is a scroll hit) or scroll to the bottom so I see the most RECENT search hits first."

Today, opening the search bar holds the reader's place (the bar's room at the head of the list is added under them), and typing a query filters the loaded rows in place with no scroll decision at all: a reader partway through the history whose current message does not match stays partway through, with the newest hits below the fold and nothing taking them there.

# Design

- On entering a typed search (the query becoming a matching query, including the first keystrokes that produce one), decide once per query change: if any search hit in the visible list is at least partly within the visible message area, keep the scroll position exactly; otherwise scroll to the bottom so the most recent hits show first.
- "On screen" means at least partly inside the shared scroller's visible message area: below the floating search bar's room at the head and above the composer/keyboard edge. When the on-screen keyboard is up, the area is the `visualViewport`-shrunken area, not the layout viewport.
- The decision applies to the main channel list; the server-hit snippet list above it and the cards below keep their current rendering (no heading change in this issue; the owner is deciding that separately).
- Reuse the page's existing scroll owner for the bottom write; do not add a second scroll writer.

# Acceptance Criteria

- With a hit at least partly on screen when the search starts matching, the scroll position does not move (phone and desktop, keyboard open and closed).
- With no hit on screen, entering the search scrolls to the bottom and the most recent hits are visible first.
- Page tests at phone (412x915) and desktop (1280x800) sizes cover both branches, in light and dark theme, with the keyboard open (visualViewport shrink) where layout is involved, and the Discuss half-pane open in at least the phone shots.
- Screenshot matrix per the standing acceptance (light/dark x keyboard open/closed x phone/desktop, before and after the targeted state), reviewed image by image.
