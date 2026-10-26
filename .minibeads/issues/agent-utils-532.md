---
title: 'shift-o-latency: Shift+O folds a message in the frame it is pressed, however big the Threads view'
status: closed
priority: 2
issue_type: bug
labels:
- vibe-talk
depends_on:
  agent-utils-480: related
created_at: 2026-10-26T14:50:42.479766186+00:00
updated_at: 2026-10-26T15:15:30.548125897+00:00
closed_at: 2026-10-26T15:15:30.548125737+00:00
---

# Description

[opus 5.5] The owner (2026-10-26): "shift-o is slow - why? It's not terribly slow but it does take a couple seconds. If I am folding up 4 messages in a row in a thread I thought I could spam the key four times and see them responsively fold up. Again I wonder if we have a clean/local model that responsively updates our UI."

Measured (Chromium, fake server, 20 threads / 128 rows unpacked in the Threads view): each Shift+O redraws the whole Threads view three times (the fold forces renderSideColumn; the Done redraws it in followDoneEverywhere and forces it again in refreshAfterInboxChange): about 220 ms of main thread per press at 393x852 and 65 ms at 1280x800, growing with every thread unpacked. A thread opened in the list costs 8-27 ms.

# Acceptance Criteria

Folding is a local change drawn in the same frame: only the thread that changed is redrawn (the fold placed on its rows in place), the Done and its write follow and are never awaited; four rapid presses fold four messages, each visible at once; a page test holds a fold to a budget and a Chromium check times each press in a large unpacked Threads view at 393x852 and 1280x800.
