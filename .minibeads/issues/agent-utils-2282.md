---
title: 'vibe-talk-client-flow-analysis: a map of what each keystroke, live event and refresh triggers inside the page, and the main-thread blockers ranked'
status: open
priority: 0
issue_type: task
assignee: mu-clientflow
labels:
- vibe-talk
- owner-ui
created_at: 2026-11-26T03:50:00-05:00
updated_at: 2026-11-26T03:50:00-05:00
---

# Description

Owner request 381 (2026-11-26 3:35 AM ET), verbatim: "Wait what. Why would slacks rate limit matter!!!!! The client is not supposed to go to slack only to the vibe talk server. Do we have any violations of that core architectural state sync principle!!!? We need to focus on that as well as perf and optimization lanes doing both benchmarking profiling and analyzing event dependencies and data and control flow inside the client. Are we completely done w our earlier typescript modularity refactor?"

This issue is part C of that request: a client flow analysis of the vibe-talk page (`vibe-talk/web/src`). For each trigger — a keystroke (composer, search field), a live event (arrival, edit, delete, read-state, older event, summary update) and a refresh or re-read — map the event and control flow from the handler to every module it reaches, the data flow (which state stores are read and written, which folds recompute), and the main-thread work per trigger (layout reads and writes, full-list passes, redraws, observers firing). Identify the blockers: anything linear in the row count per keystroke or per arrival, forced layouts inside loops, cascades where one event triggers several redraws, and timers that pile up.

It also answers the modularity question from the current tree: the `#624 voice-page-modularity` epic is closed, but the follow-up `#1523 voice-main-strict-ts` (the entry module as strict TypeScript) is still open, so the refactor is not completely done until that lands.

# Design

Static survey only; no code changes in this step.

1. Inventory the page's entry points per trigger class: keyboard and input handlers, the live-stream frame handlers, and every refresh, poll and re-read path, each by file and function.
2. For each trigger, trace the call and dependency map to every module reached, the state stores read and written, the folds recomputed, and the DOM writes, layout reads, redraws and observer firings it causes, including follow-on timers, frames and fetches it schedules.
3. Rank the main-thread blockers by estimated cost per trigger (work that grows with the row count first), each with file and function, and propose a small fix issue per blocker.
4. Share the map with the two parallel lanes: it tells `#2276 vibe-talk-client-debug-mode` which named spans matter, and it gives `#2277 vibe-talk-key-scroll-freezes-11-26` the candidate paths behind the night's key and scroll freezes without duplicating that lane's reproduction.

# Acceptance Criteria

- A report in the harness repository (`ai_docs/reports/research/2026-11-26-client-flow-analysis.md`) with a per-trigger dependency and data-flow map for keystrokes, each live-event kind, and each refresh or re-read path, every claim naming file and function.
- A ranked blocker list: each blocker names its trigger, file and function, why its cost grows (row count, folds, layout, cascades or timers), an estimated cost per trigger, and a proposed small follow-up issue.
- The modularity question answered from the current tree, including the state of `#1523 voice-main-strict-ts`.
- The span recommendations needed by `#2276` and the candidate freeze paths needed by `#2277` are named in the report.

# Notes

Owner-UI fast-track. Owning lane: mu-clientflow. Related: `#2276 vibe-talk-client-debug-mode` (the attribution instrument this map guides), `#2277 vibe-talk-key-scroll-freezes-11-26` (the night's freeze root cause), `#1976 vibe-talk-client-perf` (standing perf lane), `#624 voice-page-modularity` (closed 2026-11-15), `#1523 voice-main-strict-ts` (the open remainder of that refactor).
