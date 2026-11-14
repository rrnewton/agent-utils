---
title: 'toggle-labels-active-style: on/off toggles keep fixed labels with one shared active style'
status: closed
priority: 0
issue_type: feature
labels:
- vibe-talk
created_at: 2026-11-14T16:14:54.000000000+00:00
updated_at: 2026-11-14T16:14:54.000000000+00:00
closed_at: 2026-11-14T16:14:54.000000000+00:00
---

# Description

[watermelon-full-preview] Owner reply 024 (fast-track): the on/off toggles (Summaries, Fold read, Autoplay, Hide reader) changed their labels to say the state, so the same button read differently when on and when off. The owner asked for fixed labels with one shared active style instead, a boxed Reader section holding [Reader [Hide] [Autoplay]], and no Hide reader button on a desk. Mode switchers keep naming the current mode and are unchanged.

# Design

- Each on/off toggle keeps one fixed label; `aria-pressed` plus the shared blue outline-and-text style says whether it is on. Summaries, Fold read, Autoplay and Hide reader share that one style.
- The reader toggles sit in a boxed Reader section. A desk draws no Hide reader button.
- Mode switchers still name the current mode; only the binary on/off toggles use the fixed-label style.
- Lands with #1168 read-toggle-two-way (binary Fold read, Hide read removed, the to-do list keeping its own entry at the time).

# Acceptance Criteria

- Summaries, Fold read, Autoplay and Hide reader show the same label on and off, with the shared active style and `aria-pressed` carrying the state.
- The Reader box holds its toggles together; no Hide reader button appears on a desk.
- Mode switchers are unchanged.

# Notes

Owner reply 024. Number reserved in the host ledger; filed from the ledger row and the landing commits. Sibling #1168 read-toggle-two-way and #1470 toolbar-dynamic-promotion build on this style.

## Resolution

[watermelon-full-preview] Landed in 3ee121f6f9edbd1427ef1e4d38122dfd272032fe on main and live in release ba329b5a (with f6524cc2d1828d65b5eb42009358a68b9c75d44b for the #1168 two-way toggle). The on/off toggles keep fixed labels with the one blue active style, the reader toggles sit in their box, and a desk draws no Hide reader button. Verified as ancestors of the live release.
