---
title: 'doc-refresh-progress: Refresh fetched says what it checks, runs concurrently, skips just-checked docs'
status: open
priority: 0
issue_type: bug
labels:
- vibe-talk
- documents
created_at: 2026-11-11T15:28:12+00:00
updated_at: 2026-11-11T15:28:12+00:00
---

# Description

[watermelon-full-preview] Owner reply 363 (2026-11-11 10:21 AM ET): "Refreshing gives '39/97'... what is the scope?"

Verified on github/main: `refreshFetched` in `vibe-talk/web/src/view/documents/list.ts` re-checks the freshness of every FETCHED row among the loaded rows, strictly serially (97 was the owner's fetched-document count), refetching any whose source reports an update. `index.ts` labels the button "Refreshing done/total…" with no statement of what the total is, and a second Refresh right after the first re-checks everything again from zero.

# Design

- Say what is being checked while it runs: the button (and/or status) reads along the lines of "Checking 39/97 fetched documents for updates", so the scope is never a bare fraction.
- Run the freshness checks (and any refetches they trigger) with a small concurrency pool (about 4), reporting progress as checks complete, not as they are started in list order.
- Skip documents whose freshness was checked within the last few minutes (a named constant, in-memory for the page session): a repeated Refresh then checks only what is stale, and the completion status says how many were skipped as just-checked. A document refetched during the run counts as just-checked.
- Keep the outcome contract (`{updated, current}` plus any skipped count the status needs); tests wait on events/conditions, never wall-clock budgets (owner rule 346). Browser-check at phone (393x852 touch) and desktop (1280x800).

# Acceptance Criteria

- Refresh's progress label names the scope (fetched documents being checked for updates) with a done/total count, at both sizes.
- Freshness checks run with bounded concurrency (about 4 in flight, never strictly serial when several are due) and updated documents are refetched as before.
- A second Refresh immediately after the first skips the just-checked documents and says so; documents outside the freshness window are checked again.
- Documents view/browser tests covering the label, concurrency, and skip pass; the fix is gated at owner tier and landed on its own.

# Notes

Owner reply 363, fast-track. Filed by mu-docfetch before implementation; the fix lands separately on mu-docfetch/doc-refresh-progress. Sibling fixes: #1306 doc-star-gold, #1307 doc-fetch-visible-viewport, #1308 doc-fetch-since-prompt. Shares `list.ts`/`index.ts` with #1307/#1308; whoever lands later rebases and re-verifies the combined tree. Do not edit `web/voice.js` (#624 in flight).
