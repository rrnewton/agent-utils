---
title: 'read-mode-unify: one read mode (Show, Fold, Hide read) for every view, the Threads view''s cards included'
status: closed
priority: 1
issue_type: feature
labels:
- vibe-talk
created_at: 2026-10-22T16:08:18.578287706+00:00
updated_at: 2026-10-22T18:34:44.678857426+00:00
closed_at: 2026-10-22T18:34:44.678857206+00:00
---

# Description

[opus 5.5] The owner, 2026-10-22: "we should have a keystroke for toggling that unread only in thread list view ... I think we should unify it actually. I think that key is broken/inconsistent. Even in All view when I press it the little pop up banner message SAYS three different things including Expanded unread, collapsed unread. But the blue BUTTON on the bottom stays fixed on Show read ... the three way toggle does NOT work.. it does not actually show the ... 21 read messages ... separator lines. I propose we FIX this toggle and just make it work CONSISTENTLY for the thread view ... a thread card with any unread-children acts as an unread, and with 100% read acts as read ... in collapsed mode show separator lines that say e.g. ... 3 read threads ..." And: rename Collapse read to Fold read everywhere; Collapse then means only the size-bounded message cards, fold the run lines; keep the stored setting compatible.\n\nRoot cause: ; and : are not the read-mode key. They are #301 semicolon-expand-cycle, which expands and collapses the BODIES of long messages; "Expanded unread, collapsed read" is its notice, so the read-mode button never changed and no run lines appeared. The read mode (#221 read-modes) is key h, which already calls the button's own cycleReadMode (one state, one code path) but said nothing on screen. Coordinator, 2026-10-22: keep h as THE read-mode key, announce the mode, drive the Threads view with it, remove Unread only, reword the ;/: notices to say long messages.

# Design

Read modes renamed: show, fold (was collapse; a stored collapse maps to fold), hide; labels Show read, Fold read, Hide read. h announces the mode on the status line. ;/: notices say long messages. Threads view: Unread only (#259) removed; a card is unread when any of its thread's messages (root or replies, held or reported) is unread, read when all are, packed or unpacked. Show: everything. Hide read: read cards go with their unpacked rows; inside an unread unpacked thread read rows hide as before; Main channel rows as Main. Fold read: runs of consecutive read items among the cards (a card with its unpacked rows, or a Main channel row) fold into one line, "... 3 read threads ..." (and "... and 2 read messages" where Main rows join), opened by a tap or Enter; read rows inside unpacked threads fold into the usual lines. One rule (readRunPlan, placeInReadRun, readRunLine), no Threads-only copy.

# Acceptance Criteria

Key h and the button agree in Main, All, a thread and the Threads view, h says the mode; Fold read via h draws run lines; thread runs count right; a thread with one unread reply is unread; Unread only gone and the heading is Main channel, Unpack msgs, order; Fold read wording everywhere, old stored value honoured. Page tests and Chromium at 1280x800 and 393x852.
