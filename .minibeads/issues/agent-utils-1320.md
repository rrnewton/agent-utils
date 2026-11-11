---
title: 'no-freshness-band: the channel list reserves no header band; the freshness pill floats over the list'
status: closed
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

## Resolution

[watermelon-full-preview] Removed the reserved band: the `#screen-main:not([data-searching]) > #scroll-area:has(> #pane-discord[data-freshness])` margin rule in `web/styles/voice/09-to-do-view.css` is deleted and `#pane-discord` no longer carries static `data-freshness` in `web/voice.html`, so with search closed the first message sits at the top of the scroll area and the freshness pill floats over the list on its zero-height anchor — showing, clearing or rewording it takes no layout room, so #1210's no-shift holds with nothing held statically. Search still opens only behind the floating magnifier, and only the open bar makes room at the head. `freshness.ts` comments updated to match; no scroll writes added (landings still measure the pill as drawn via `floatingClearance`). New no-band test in the existing `tests/js/page/search.mjs` (with `sync.mjs`/`rows.mjs` assertions updated from the old reservation), and the Chromium walks `live_pill_browser.py`/`offline_cache_browser.py` updated to the floating-pill contract (the latter also samples the reply bridge's line along its segment, after a one-device-pixel corner sample missed a drawn line at the new head position). Verified: focused page tests, the #1210 `scroll_soak_browser.py` (exit 0), `live_pill_browser.py`, all three `offline_cache_browser.py` profiles, and desktop 1280x800 + phone screenshots inspected — no band, pill floating, magnifier floating.
