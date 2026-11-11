---
title: 'no-freshness-band: the channel list reserves no header band; the freshness pill floats over the list'
status: open
priority: 0
issue_type: bug
labels:
- vibe-talk
- owner-ui
created_at: 2026-11-11T16:32:07+00:00
updated_at: 2026-11-11T16:32:07+00:00
---

# Description

[opus 5.5] Owner request (reply 389): "Before we had a floating magnifying glass icon that only popped open to a search bar when activated. Now we seem to have a persistent header bar that cuts into our vertical space..... why did that change? I see the \"Showing messages from HH:MM\" chip lives in that header but that is designed as a floating chip anyway...."

Cause: 5de62d18 (#1210 ui-stability-no-async-shift, D1) made `#pane-discord` carry `data-freshness` statically in voice.html. The rule in styles/voice/09-to-do-view.css that adds `margin-top: calc(var(--float-line) + max(var(--freshness-box), var(--search-disc)) / 2)` to `#scroll-area` therefore applies whenever the pane is shown, so the head of the list keeps a permanent band for the freshness pill. Also check 4a642823 (the searchopen.ts move) and 623f4f13 (the freshness.ts move) in case the search strip itself changed.

# Design

- No reserved band. The freshness pill floats over the list as an overlay that takes no layout room, so it appearing, clearing or being reworded still causes no layout shift (#1210 still holds).
- Search stays behind the floating magnifier and opens only on demand.
- Any scroll the change needs goes through the single scroll owner (#1251).

# Acceptance Criteria

- With search closed, the head of the channel list reserves no band: the first message sits at the top of the scroll area whether or not freshness is showing.
- The freshness pill is drawn over the list; showing, clearing or rewording it moves nothing (the #1210 no-shift soak and its test stay green).
- A new test checks that the list head reserves no band while search is closed.
- Owner tier, path-selected gate, deployed on landing.

# Notes

Related: #1210 ui-stability-no-async-shift, #32 freshness-pill-overlap, #1251 single scroll owner.
