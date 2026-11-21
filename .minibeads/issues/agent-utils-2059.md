---
title: 'vibe-talk-ui-latency-architecture-review: the page reacts in its own frame, audio and server follow'
status: open
priority: 0
issue_type: task
labels:
- vibe-talk
- ui
- architecture
- performance
created_at: 2026-11-21T10:47:00-05:00
updated_at: 2026-11-21T10:47:00-05:00
---

# Description

[watermelon-full-preview] Owner request 284, architecture review (2026-11-21). The owner: user-facing components must react quickly and deterministically to every UI action; text-to-speech must have an asynchronous relationship to those fast components; audio latency should be optimised, but never at the cost of redraw and button-press latency. The TypeScript module re-architecture (#624 voice-page-modularity) should be checked for completeness, and component dependencies reviewed for lower latency and for decoupling of different latencies.

The governing design note lives in the harness repository at `ai_docs/plans/2026-11-21-vibe-talk-ui-latency-architecture.md` (with its changelog entry), audited at this repository's `93cf0b15`. It is a design note first: the owner reviews it before any restructuring. Its findings in brief: import layering is clean (464 files, 0 cycles or direction violations by the kept script `vibe-talk/web/import_graph.mjs`), input handlers already almost never await the network, and the remaining violations concentrate in speech start gating first audio on serial prepares and fixed cushions, three fetch-gated first paints (documents reader and list, empty view switch), same-task screen switching, and synchronous whole-list redraws with per-row layout reads. #624 voice-page-modularity is organisationally landed; the 9,908-line non-strict `src/app/main.js` composition root remains by design, tracked for strict conversion as #1523 voice-main-strict-ts.

# Design

- Target model (design note section 4): an in-memory UI store is authoritative and its reducers are synchronous; the speech engine and server sync are asynchronous followers (subscriptions, cancellable commands carrying generations, stale results dropped). One checked rule: a module that can run inside an input task or a draw may not await network, audio, or another follower; followers may await but may not draw.
- Staged migration S1 to S6, biggest latency win first, each step independently landable: S1 speech follows the paint (rides #2058 vibe-talk-reader-arrows-instant), S2 no fetch-gated first paints, S3 shorter press tasks (with the parked #2042 vibe-talk-scroll-style-recalc containment step), S4 per-keystroke costs (#1976 vibe-talk-client-perf budgets), S5 reads stop serialising behind each other (including the stream-attach stale-live fix), S6 structural decomposition of the composition root and #1523 voice-main-strict-ts.
- Enforcement: extend `vibe-talk/web/architecture.mjs` sayings in place (no second checker), and add a one-frame event-count gate in the scroll/search gate family (paint asserted before the next frame while the follower fetch is still pending; counts only, never wall-clock, per owner rule 346).

# Acceptance Criteria

- The design note is committed in the harness repository with its changelog entry, and this issue plus the kept script `vibe-talk/web/import_graph.mjs` are landed in this repository. (This issue's own landing; restructuring stages get their own issues after owner review.)
- The owner has reviewed the note and answered its three decisions (whether the presentation layer stays advisory, whether Documents may paint held content, and stage sequencing); the answers are recorded here or in the note.
- Each approved stage S1 to S6 is filed as its own issue citing this one, with fixture-only measurement (never the live page) and event-count gates in the existing style.

# Notes

Related: #624 voice-page-modularity (closed; its checker is the enforcement home), #1523 voice-main-strict-ts, #1976 vibe-talk-client-perf (owns the budget table this extends), #2034 vibe-talk-scroll-layout-reads (closed; the one-frame gate's shape), #2042 vibe-talk-scroll-style-recalc (containment follow-up folded into S3), #2056 vibe-talk-reader-retry-and-error-telemetry (retry must live inside the speech follower), #2058 vibe-talk-reader-arrows-instant (S1's first step; do not duplicate).
