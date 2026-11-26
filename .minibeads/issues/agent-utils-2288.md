---
title: 'vibe-talk-arrival-full-search-reply-arrow-pass: one live arrival runs a search, reply-link and reply-arrow pass over every row'
status: open
priority: 1
issue_type: bug
labels:
- vibe-talk
- performance
created_at: 2026-11-26T06:25:00-05:00
updated_at: 2026-11-26T06:25:00-05:00
---

# Description

Found by the attribution for `#2277 vibe-talk-key-scroll-freezes-11-26`. It confirms, on a page whose arrivals really land rows, the suspect `#2282 vibe-talk-client-flow-analysis` left open as (b) "per-arrival triple pass" (blockers B3 and B5 in its ranking): the client flow report could not exercise it because its clone never landed an arrival.

Each live message on the channel shown runs `receiveLiveMessage` (`vibe-talk/web/src/view/channel/livestream.ts`) -> `appendChannelRow` (`view/channel/readcore.ts`, once per column) -> `applySearch` (`view/channel/searchrows.ts`), whose settled pass draws the reply links and the reply arrows for the whole list: `renderReplyLinks` (`view/channel/gathering.ts`) walks every row of `#discord-log`, and `measureReplyArrows` (`view/channel/replyarrows.ts`) measures them, both reaching row parts through `childByClass` (`view/dom/tree.ts`). The `#2200` same-task guard does not help here: each arrival is a new task.

Measured by the `#2277` lane on a local server running a page build with about 730 rows (about 37,800 elements), phone size, CPU throttled 4x, with every `:has()` rule disabled (so none of this is the `#2283` cost), in the worst long task (250 ms), forced style and layout by JavaScript caller:

| Caller | Forced style/layout |
|---|---:|
| `childByClass <- renderReplyLinks <- applySearch` | 48 ms |
| `childByClass <- measureReplyArrows <- renderReplyLinks` | 39 ms |
| `gutterPiece <- renderReplyLinks <- applySearch` | 9 ms |
| `measureReplyArrows` itself, `drawSeams <- applySearch`, `saveChannelScope <- appendChannelRow` and smaller | about 8 ms |

About 105 ms of forced reads per arrival, plus `childByClass` at 62 ms of self time, all growing with the number of rows in the list, not with the one row that arrived.

# Design

Per-arrival work proportional to the rows that changed:

1. An arrival appends or replaces one row (or one card). Draw the search dressing, reply link and reply arrow for that row and its neighbours only (the row above it, which may gain a link, and the reply's parent when it is in the list), not a pass over every row. Keep the full pass for the changes that really change every row: a new search, a filter or view change, a redraw.
2. Read layout once per pass and write after: `renderReplyLinks` and `measureReplyArrows` interleave reads (`getBoundingClientRect`, `childByClass` lookups that touch layout) with writes, so each read can force a fresh layout of a list already made dirty. Batch the reads for the rows being drawn, then write.
3. Reuse the row parts `childByClass` already caches (`#532`) across the pass instead of walking again for rows whose parts did not change.
4. Coordinate with `#2282`'s other arrival blockers (the whole-cache rewrite and the arrival -> re-read cascade) so one arrival does not pay for the same row three times.

# Acceptance Criteria

- A counting browser check (no wall-clock bound; rule 346): on a channel of 200+ rows at 412x915 and 1280x800, one live arrival visits a bounded number of rows in `renderReplyLinks` and `measureReplyArrows` (the arrival, its neighbours and its parent: a constant, independent of the list's length), counted through a test hook; the same check with 2x the rows gives the same count. A new search or a view change still visits every row.
- Reply links, reply arrows, search marks and seams look exactly as they do today after arrivals, edits and redraws (the reply-arrow and search browser walks stay green; the screenshot matrix unchanged).
- In the `#2277` attribution trace (same page build and row count, `:has()` rules disabled), no forced style/layout attributed to `childByClass <- renderReplyLinks` or `measureReplyArrows` exceeds the per-row cost of the rows the arrival changed.

# Notes

Related: `#2282 vibe-talk-client-flow-analysis` (suspect (b), blockers B3 and B5; report: https://github.com/rrnewton/dev-agent-utils/blob/main/ai_docs/reports/research/2026-11-26-client-flow-analysis.md), `#2277 vibe-talk-key-scroll-freezes-11-26` (the attribution that measured it), `#2283 vibe-talk-root-has-rules` (the separate whole-document style cost, ruled out here by disabling those rules), `#2276 vibe-talk-client-debug-mode` (spans to watch this in the owner's own traces), `#2200` (the same-task guard on the settled pass), `#532` (the row-parts cache in `childByClass`).
