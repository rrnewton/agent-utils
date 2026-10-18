---
title: 'settings-multi-column: on a wide desktop window, tile Settings'' sections into columns'
status: closed
priority: 2
issue_type: feature
labels:
- vibe-talk
created_at: 2026-10-18T20:43:26.565867201+00:00
updated_at: 2026-10-18T22:31:17.414160940+00:00
closed_at: 2026-10-18T22:31:17.414160789+00:00
---

# Description

[opus 5.5] Owner, 2026-10-18: "on wide desktop windows, tile Settings' top-level panes into width-dependent columns, each pane kept whole and packed, order and focus order unchanged; phone/narrow single column." The number of columns follows the window's width; a section is never split between two columns; the sections stack with no gaps under shorter ones; the reading order and the Tab order stay the order of the sections as they are today. A phone, or a desktop window too narrow for two columns, keeps the one column it has now. Page tests (the rule applies only to a wide desktop window, the order is unchanged, nothing reorders the focus) with negative controls, and Chromium screenshots at 393x852, 1280x800 and 1920x1080.
