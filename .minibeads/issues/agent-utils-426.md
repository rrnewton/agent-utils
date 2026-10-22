---
title: 'threads-sort-toggle: order the Threads view''s cards by latest update or by thread creation'
status: closed
priority: 1
issue_type: feature
labels:
- vibe-talk
created_at: 2026-10-22T14:18:50.042170004+00:00
updated_at: 2026-10-22T16:06:12.204011637+00:00
closed_at: 2026-10-22T16:06:12.204011547+00:00
---

# Description

[opus 5.5] The owner, 2026-10-22: "One toggle I DO think we should have at the top of the thread view is to order by thread creation vs latest update. It could be a drop down on sort order or just a toggle button that changes like the Collapse Read/Show Read button. Our other buttons up there are toggles so maybe start with that and if we add more sort orders make it a drop down."

# Design

A chip in the Threads view heading saying the order the cards are in: Latest update (default, where each thread last moved) or Thread created (where its first message was posted); a press or key d goes to the next order, kept on this device, aria-pressed true off the default, aria-label naming the order. The orders are a table (THREAD_ORDERS: label, short label, time function), so a third order is one entry and the chip can become a menu. Main channel rows (#419) stand among the cards by their own time against each card's time in the chosen order. Switching keeps the selected card in view. Only over the Threads view's cards; two columns keep latest update.

# Acceptance Criteria

Both orders draw the cards (and Main channel rows) in the right order; the choice persists across reloads; key d and the chip agree, both in ? help; the selected card stays in view on a switch. Page tests with negative controls; Chromium at 1280x800 and 393x852.
