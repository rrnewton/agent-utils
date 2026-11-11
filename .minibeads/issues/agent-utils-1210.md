---
title: 'ui-stability-no-async-shift: nothing the reader is looking at moves unless the reader acted'
status: open
priority: 0
issue_type: bug
assignee: mu-stable
created_at: 2026-11-10T14:34:23.000000000+00:00
updated_at: 2026-11-10T14:34:23.000000000+00:00
---

# Description

Owner report (reply 298, 2026-11-10 9:08 AM ET, Android after refreshing): "The experience is bad because it seems jumpy still. I'm interested instability across the UI. I don't want various elements jumping and tweaking asynchronously. Right now I can scroll to a point and start reading and then pretty frequently I see something cause what I'm looking at to move. I don't understand what all these events are that are causing these problems. And I don't understand why it seems to have gotten worse and wire fixes have been effective yet."

The requirement is UI stability: nothing the user is looking at moves unless the user acted. No background event (a new message arriving, a reaction, read state, a live-tail edit, a recount, a chip appearing) may move the viewport or anything inside it.

Companions, owned separately: #1208 scroll-trace-layout-shift makes every shift visible from the device, and #1209 selection-survives-updates stops background updates destroying a text selection. This issue owns the page-structural audit, the automated stability test, and the page fixes that test finds.

# Design

Work in the vibe-talk page (`vibe-talk/web`), coordinated through the coordinator so page code is not edited under another lane unannounced:

- Why worse: list the vibe-talk deploys of the last day and, for each, any asynchronous height or position change it adds (reaction overlay remeasure, document-kind cards and buckets, the Newest recount, the raw-message row, the scroll trace itself, the chip-height snap observers, the view refactor pieces). Rank the suspects from the code, not from impressions.
- Structural audit: whether the page fights the browser's native scroll anchoring (any `overflow-anchor: none` on a scroller, manual `scrollTop` writes that undo the browser's anchoring), every scroll writer and whether it can run while the reader is mid-channel, and every element whose height can change asynchronously and whether its space is reserved before it loads. Decide whether relying on native anchoring plus reserving space removes a whole class of jumps.
- Automated stability test (the core deliverable): load a long channel, scroll to the middle, then drive background updates for 60 s (new messages arriving, reactions, read state, live-tail edits, the Newest chip, the draft chip, the periodic re-read). Assert approximately zero layout shift in the viewport (layout-shift entries) and an unchanged anchor-row position, at phone (393x852, touch) and desktop widths, as a gate check. It is expected to fail today; it lands as a known-red check only on the coordinator's word, after a first report naming which updates move the viewport.
- Fixes: fix the causes the test finds, one small change at a time, each keeping the stability test as its proof.

# Acceptance Criteria

- A ranked suspect list for the recent deploys, each entry naming the mechanism and the file and function.
- A structural audit naming every scroll writer, every asynchronous-height element and whether its space is reserved, and whether the page relies on or fights native scroll anchoring.
- The automated stability test exists as a gate check at phone and desktop widths; with the reader parked mid-channel, 60 s of background updates move the anchor row by at most 1 px and add approximately zero layout shift, or the test's failure names which update moved it.
- Each fix this issue lands leaves the stability test covering its cause, so the same shift cannot return silently.

# Notes

Owner reply 298, 2026-11-10. Slot stable, branch mu-stable/ui-stability. The device instrumentation and the selection-loss fix (owner reply 296) are #1208 scroll-trace-layout-shift and #1209 selection-survives-updates; coordinate page edits through the coordinator.

## Completed by mu-stable (2026-11-10, landed b295042a)

[watermelon-full-preview] Parts A, B, C and D1/D2 of this issue are done; the issue stays open for #1251 scroll-single-owner.

- A (why worse): ten deploys in ~14 h ranked from deploy history + code. Top: chip-row ResizeObservers (Newest recount/draft/read strip, re-gated by #1185), reaction-overlay demotion (#1063), placing read, raw-row/rest redefinition (#1135), reply-count re-reads. #1166 excluded (not landed); scroll trace inert when off.
- B (structural audit): native anchoring is ON for both scrollers (no overflow-anchor opt-out) and fought by manual capture-correct writers (scrollkeep/readcore), composing only when both pick the same anchor. Full writer and async-height inventory reported to the coordinator; redraw model published in the harness design note for owner reply 300 (one render loop, one scroll owner, reserve space; no framework now, Preact only if convention fails).
- C (soak): vibe-talk/tests/scroll_soak_browser.py, gate check vibe-talk.check.scroll-soak-browser, report-only (--strict asserts; TODO flip when part D holds it green). Summaries OFF (owner setting), full 60 s: anchor within 0.4 px mid-Main at 393x852 and 1280x800; near-foot slack snap 18 px on a thread reply at both sizes (with mu-scroll); desk thread-card resort shift 0.068; freshness pill 0.0032/0.0007 per refresh. Summaries ON (--summaries on): summaries-landing shift 0.2448 phone / 0.2173 desk (content 66-84 px up) before D2's reserve, which was reverted for defeating summarised compactness (screenshots state 32); batching stands, shift back at baseline when on. Owner has summaries off (replies 308/309; page summaryTargets and server summaries_off both gate fully).
- D1 landed: freshness room held statically (data-freshness in markup, reserveFreshnessRoom deleted) so the scroller never resizes for freshness.
- D2 landed in part: summary answers redraw once per frame (scheduleRenderSummaries in folding.ts). Height reserve reverted (above); holding summarised height without losing compactness needs an owner decision.
- Device traces (#1208, auto-sent): mutate-dominated (row replace-redraws), write cluster preservingScroll then settleAfterRead at ±112 px around one read landing. D4 one-correction-per-frame (holdingMessage/settleAfterRead + shared mark in scrollkeep; boundary agreed with mu-scroll, selection capture rides alongside) moves to #1251 scroll-single-owner (mu-renderloop). D3 reaction-overlay decide-before-paint waits on #1209/#1212. Also noted for part D from mu-scroll's ranking: selection/quote chip repositioning 11-17 px, discord-log 36 px, scroll-tools/composer 20 px.
- Follow-up filed: #1215 collapse-anchor-below (parked).
