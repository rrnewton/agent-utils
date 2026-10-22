---
title: 'thread-card-pack-toggle: pack and unpack each thread from its card, and Unpack in three states'
status: closed
priority: 1
issue_type: feature
labels:
- vibe-talk
created_at: 2026-10-22T18:35:58.369181641+00:00
updated_at: 2026-10-22T22:58:45.135646855+00:00
closed_at: 2026-10-22T22:58:45.135646745+00:00
---

# Description

[opus 5.5] The owner, 2026-10-22: "I want to toggle pack/unpack on different threads the same way I toggle collapse/expand on messages. Currently CLICK on a thread card in thread list view lights up that card with its colored backend and dims the colored left edge of all the other threads. That's a behavior from the interleave All view that really doesn't make sense in the thread view ... For now I propose a 3 state toggle. Collapsed, expanded card but no children, expanded card and expanded children ... in the same place we put that little x to break the connector bridge, we could have a little up triangle to pack those messages back into the card." And: "let's also go 3-way with unpacking in thread list view ... The middle state will be to pack read threads and unpack any with non-zero unread child messages ... ONLY in this middle state I would also like to render as grey the colorful left sidebar of the READ cards." Coordinator: the pack control is an up-pointing triangle icon sized like the bridge's x, aria-label Pack thread, with a tooltip.

# Design

Threads view only: a click (or Enter, o) on a card cycles its thread: collapsed card -> card shown whole, nothing unpacked -> card whole with its messages unpacked (#404 blocks, bridge, read modes) -> collapsed; a summarized card (#405), written to fit, skips the middle state. No highlight and no dimming there (two columns keep #277). The Unpack chip cycles Pack all -> Unpack unread (threads with any unread message unpacked, read ones packed; #432's rule) -> Unpack all, labelled with its state, aria-pressed false/mixed/true, key u, kept per device (an old on/off maps to all/packed). A per-thread choice overrides the chip for that thread until the chip is pressed again; choices kept per device. Only in Unpack unread, a read card's colour strip is grey by the highlight's own dim rule. A thread read while in Unpack unread packs in place (the card kept where it is) and stays. An up triangle (aria-label Pack thread, tooltip) in the first unpacked row's gutter, where All's bridge has its x, packs that thread with its card kept in place; key ^.

# Acceptance Criteria

Page tests: the three card states (and two for a summarized card), click and Enter; no highlight or dim in the Threads view; the chip's three states, label, aria-pressed, key u, storage and the old values; per-thread overriding the chip until pressed; the grey strip only in Unpack unread; a thread packing in place when read; the triangle and ^ packing with the card in place. Chromium at 1280x800 and 393x852.
