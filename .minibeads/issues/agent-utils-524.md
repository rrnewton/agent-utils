---
title: 'thread-bridge-even-connectors: own unpacked rows left-justified so every bridge connector is the same length'
status: open
priority: 2
issue_type: feature
labels:
- vibe-talk
created_at: 2026-10-25T21:41:30.794614797+00:00
updated_at: 2026-10-25T21:41:30.794614797+00:00
---

# Description

[muse watermelon] Owner, 2026-10-25: "I don't like the variable length thread bridge connectors. Right now we've made MY messages less wide and right-indented them. Let's have them left-justified so the colored line is the SAME length. We already have a color distinction for mine/theirs messages, and if we keep the current width for my messages we'll have a ragged edge on the right, but at least we won't have these variable length connectors." In an unpacked thread the owner's rows begin half their old width difference further in than the thread's other rows (#492 threads-row-widths), and their bridge piece is widened to reach back to the spine, so the horizontal connector has two lengths. Put the owner's unpacked rows at the same left edge as the thread's other rows, keeping their current narrower width (the difference moves to the right edge, which goes ragged), so every connector is the same length; the mine/theirs colour distinction stays. Check every place threads unpack or replies gather (Threads view cards, two columns, All, Main) on phone and desktop, and that nothing else relied on the extra left indent (receipts, sending cards, reply boxes, the centred own main row's step from #424/#501).

# Acceptance Criteria

At 393x852, 412x915 and 1280x800, in an unpacked thread holding the owner's and others' rows, every row's left edge and every bridge connector's length agree within a pixel or two; the owner's rows keep their narrower width and their colour, their right edge short of the others'. Gathered replies in All/Main keep one left edge and one connector length. Existing bridge, pack, reply-box, sending-card and centring checks stay green. Page tests (Chromium geometry at the three sizes, with screenshots) pin it.
