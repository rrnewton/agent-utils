---
title: 'vibe-talk-search-bucket-chips-pinned: bucket chips stay under the search bar, plainly pressed or not'
status: closed
priority: 1
issue_type: bug
labels:
- vibe-talk
- search
- ui
created_at: 2026-11-19T04:42:21-05:00
updated_at: 2026-11-19T12:59:27-05:00
---

# Description

[watermelon-full-preview] Owner (thread 216, relayed by the liaison 2026-11-19): the extra search buckets must be toggled "WITH A CHIP under the search bar". Today the chips ("1 in main", "2 in threads") sit at the TOP OF THE RESULTS LIST and scroll away; in the #1894 evidence shot 1 they are not visible at all. Also, the pressed and unpressed chips look the same (shot 2: you cannot tell which is pressed).

The bucket row `search-bucket-row` in `vibe-talk/web/voice.html` stands inside `#scroll-area` > `#pane-discord`, at the head of the scrolling list, so it leaves the screen with the first scroll. Its chips (`search-bucket-*` in `vibe-talk/web/src/view/channel/olderwalks.ts`, `renderServerSearch`) set `aria-pressed` correctly, but the only pressed styling is `font-weight: 700` and the shared `.chip[aria-pressed="true"]` tint, which the owner cannot tell apart at a glance in light or dark.

# Design

- Pin the bucket chips in the search panel directly under the search field/filter row, so they stay visible however far the results scroll, on phone and desktop, with the on-screen keyboard open or closed. The row leaves the scrolling list for the panel that holds `search-float`; the `scroll-area` head room grows to cover the bar plus the chip row when a search is open, so no result is hidden underneath and no result moves when the counts arrive.
- Give the pressed state a clearly different look in light and dark: a filled background plus a check or `aria-pressed` styling at AA contrast, worn from `aria-pressed` so the four chips (thread, main, threads, transcripts) all share it. Unpressed stays quiet on the panel; pressed reads at a glance.
- Keep #1894's behaviour: buckets start OFF at every new search (`resetBuckets`, home bucket only), and a late bucket landing never nudges the scroll after the reader moved (`placeFilterAnchor` / `ScrollKeeping.userMovedSince`; landed as 253ff5ab/747cd7a3). The chip counts update as rounds land, without layout jumps: the row keeps its stable one-chip height while an answer is pending.

# Acceptance Criteria

- On a real server, a search with thread hits: chips are visible under the bar after scrolling the results to the bottom, on phone and desktop, keyboard open and closed.
- One chip pressed, one not, plainly distinguishable in light and dark; `aria-pressed` matches the look on every bucket chip.
- A new search: all extra chips unpressed again (home bucket only, as #1894), and a late bucket landing after the reader scrolled changes no scroll position.
- Counts update as rounds land without layout jumps; existing bucket, server-hits and place-keeping behaviour otherwise unchanged; the Whole-threads toggle and Labels filter compose as before.
- Dynamic proof per the standing rules: end-to-end through the real server router with the page's exact requests (only external upstreams stubbed), screenshots of the targeted states before from main `8d190338` and after, light and dark x android-412 and laptop-1280 plus keyboard-open phone, with BUILD.txt in the harness format, and the liaison's image review before announcement.

# Notes

Follow-up to #1894 vibe-talk-search-extra-hits-off-by-default. Coordinates on the same code: `searchBucketDefs`/`renderServerSearch` in `vibe-talk/web/src/view/channel/olderwalks.ts`, `listcontrols.ts` chip listeners, `voice.html` `search-bucket-row`, and the bucket-row / chip rules in `11-message-text.css` and `12-channel-rows.css`.

## Resolution

[watermelon-full-preview] Landed as 5730d703 on github/main. The bucket row moved out of the scrolling list into the search panel directly under the search field/filter row (#channel-freshness-anchor, outside #scroll-area), on an opaque panel ground with its head reserved while a chip stands, so the chips stay visible after scrolling to the bottom at 412x915 and 1280x800, keyboard open and closed; a pressed chip wears a filled accent ground with a check from aria-pressed at AA contrast in light and dark, unpressed quiet. #1894 held: extra buckets start off at every new search, late landings keep the scroll. Also folded in the liaison's two shot findings: the pinned row's opaque backing (no scrolled content through it) and the author-filter tooltip as a real page bug — edge-aligned inside the viewport and never raised by sticky touch hover. Evidence: pinned walk 5/5 plus page suite 2564/2564 on the final tree and on main, gate pass on the gated tree with the overlap verified on the combined tree; shots in /tmp/mu-chippin-shots-after with BUILD.txt, before in /tmp/mu-chippin-shots-before, scene 80 before/after in /tmp/scene80-check and /tmp/scene80-after.
