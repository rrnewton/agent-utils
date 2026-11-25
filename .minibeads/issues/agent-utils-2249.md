---
title: 'vibe-talk-search-signpost-hidden-while-reserved: the On this screen signpost stays hidden while the search bucket head is reserved, so no row moves in the debounce window'
status: open
priority: 1
issue_type: bug
labels:
- vibe-talk
- search
- owner-ui
created_at: 2026-11-25T13:46:00-05:00
updated_at: 2026-11-25T13:46:00-05:00
---

# Description

Found by #2200 vibe-talk-search-tab-counts-match-cards in its per-frame evidence, and pre-existing on main: with "lantern" settled (bucket chips standing, the list's head reserved), typing a key ("lanter") moves a row the reader is looking at +23 px down for one debounce window (about 220-250 ms after the key), then back to 0.0. Owner request 370 is against exactly this kind of movement ("I want my message to stay on the screen and stable").

Cause: the `#1913` rule hides the "On this screen" signpost (`#search-screen-sep`) only while a bucket chip stands. The typing-fast pass retires the chips inside the input event while `#2200`'s head reservation (`data-bucket-head` on `#screen-main`) keeps its room, so the signpost reappears at the head of the list for the window, then hides again when the answer stands the chips back up.

# Design

- Hide the signpost while the bucket head is reserved (`#screen-main[data-searching][data-bucket-head]`), in the same rule as `#1913`'s chip condition: one CSS selector, no script change. The reservation already stands whenever a chip stands, and while an answer is pending.
- Settled with no chip (no hit anywhere), the reservation is released through the scroll owner's hold and the signpost shows exactly as it does today; the release and the signpost's return happen in the same mutation, under the same hold.

# Acceptance Criteria

- A page test: with the chips standing, a new key typed (the answer pending, the head reserved), the signpost takes no layout box; settled on a query with no hit anywhere (no chip, no reservation) it shows.
- A per-frame trace (rAF-stepped) of a row fully below the chip panel, "lantern" settled then "lanter", phone 412x915 and desktop 1280x800, light and dark: 0 px at every frame on the fixed build, and the +23 px window on main as the negative control, with a JSON sidecar.
- Shots of the settled no-chip state (no change against main) and of the pending state, plus the standard screenshot matrix, each folder with its BUILD.txt.

# Notes

Owner-UI fast-track (rule 286). Related: #1913 vibe-talk-search-typing-lag (the signpost rule), #2200 vibe-talk-search-tab-counts-match-cards (the head reservation), #1899 vibe-talk-search-bucket-chips-pinned, #1762 vibe-talk-server-hits-collapsed-line (the signpost).
