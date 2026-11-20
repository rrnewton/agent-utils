---
title: 'vibe-talk-thread-picker-overhaul: overhaul how the thread picker popup looks'
status: open
priority: 0
issue_type: feature
labels:
- vibe-talk
- ui
created_at: 2026-11-18T23:46:38-05:00
updated_at: 2026-11-18T23:46:38-05:00
---

# Description

[watermelon-full-preview] Owner request, thread 215 (2026-11-18 7:13 PM PT): "I still want an overhaul on how that thread picker looks... it's ugly". Earlier (thread 212): "That pop up list needs reformatting anyway. It looks ugly and is inexplicably in a different font than the normal messages."

The thread picker popup (`#1550 thread-list-pins`) lists the fixed views (Main, All, Thread list, Two columns, Transcripts) and up to eight threads with a pin button, a facts line (age and reply count), a slug and a title. It has no thread colours, no unread counts, no group headings, and no current-view marker, so pinned and recent threads read as one flat list. Owner-priority, deployed on landing.

# Design

Step 1 (before building): 2-3 quick mockups as screenshots of real-route variants on a test instance with real seeded threads, phone and desktop, light and dark, each with a one-paragraph rationale, sent to the coordinator for the owner to pick. Step 2: build the pick.

Owner thread 277 replaced the mock-up pick with a direct spec (verbatim in the coordinator's brief): the primary fixed options visible and bigger at the BOTTOM of the popup, close to the thumb on mobile, their names centred and not indented; the threads above them in a scrollable sub-pane sorted chronologically like the messages, most recently modified at the bottom, opening scrolled to the bottom. The earlier overhaul goals that still fit stay: the messages' own font, hierarchy, thread colours, unread counts, slugs. Pins still mark and toggle their thread; they no longer reorder the list. Keep to the picker popup's own markup and CSS (`drawThreadPickerPopup` in `web/src/view/channel/threadlist.ts` and the `.thread-picker-*` rules in `web/styles/voice/18-control-bar.css`): what the picker lists is another lane's question (#1886), and the search bar is another lane's (#1853).

# Acceptance Criteria

- The fixed options stand at the bottom of the popup, bigger and better decorated, with centred labels; the threads above them stay a scrollable sub-pane, chronological with the most recently modified at the bottom, and the pane opens scrolled to the bottom.
- The popup uses the message font and shows thread colours, unread counts and slugs, marks the current view, and stays readable at 412px.
- Pin toggling and persistence, the native menu for keyboard use, and Escape closing the popup all still work; tests cover the order, the bottom bar, the centred labels, the opening scroll and keyboard use.
- The feature is exercised dynamically on a test instance through the real server before landing, with phone 412 and desktop 1280 shots in light and dark captured with the shared harness and inspected.

# Notes

Prior work: `#194 thread-picker-polish` (the picker's age and count facts), `#1550 thread-list-pins` (the popup itself, its pin buttons and server pin store), `#1563` (slug plus full title in each row). Coordination: #1886 changes what the picker lists (transcripts hidden) and #1853 touches the search bar; this lane keeps to the popup's own markup and CSS.
