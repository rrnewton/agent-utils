---
title: 'vibe-talk-thread-search-older-hit-above-reader: an older in-thread hit placed above a reader at a short thread''s head moves the row being read'
status: open
priority: 2
issue_type: bug
labels:
- vibe-talk
- owner-ui
created_at: 2026-11-25T15:58:00-05:00
updated_at: 2026-11-25T15:58:00-05:00
---

# Description

Pre-existing, same on main. Found while tracing #2256 vibe-talk-search-head-room-no-shift. In a thread's own view that fits on screen (the reader at its head), a search for words the thread says ('lantern') lands the server's answer about 430 ms after the key; the answer places an older in-thread hit (a reply the page had not loaded) as a card ABOVE the first loaded result, and the row being read moves down by the card's height: +103.9 px at 412x915 and +114.9 px at 1280x800 on main (+75.9 px on desk with #2256, whose head room is no longer part of it). Measured on the typing-fast fixture (`tests/search_typing_fast_browser.py` BigApi, the lantern thread, its older reply id 151), per frame from the key; probe and JSON sidecars in the #2256 evidence (`headroom_trace.py`, reader 'top'). Owner request 370: nothing may move the row being read.

# Design

- A card the answer places above the row being read keeps that row's top: the placement's hold (`preservingScroll` around `placeServerHits`, `#1901`) holds the reader's row for a list that fits as it does for one scrolled into, instead of following the foot or leaving the head.
- Where the card goes above a reader at the head of a list that fits, the room it needs comes from foot room (`#2076`) or the hold, not from the reader's row moving.
- The first result still never starts under the chip panel (#2254's rule); if the two collide, say which wins in the design before coding.

# Acceptance Criteria

- A per-frame trace from the key through the settled answer in a short thread view with an older in-thread hit, phone 412x915 and desktop 1280x800, light and dark: the row being read keeps its top (dy 0 within 1 px); main's +103.9 / +114.9 px as the negative control, with a JSON sidecar.
- A page test for the placement in a list that fits.
- Shots before and after the answer, each folder with its BUILD.txt.

# Notes

Owner-UI (owner 370). Filed at the coordinator's request from the #2256 traces. Related: #2256 vibe-talk-search-head-room-no-shift, #1901 vibe-talk-search-late-hits-shift-anchor, #2254 vibe-talk-search-chips-cover-first-card, #2258 vibe-talk-thread-search-park-first-pass.
