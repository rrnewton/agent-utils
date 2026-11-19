---
title: 'vibe-talk-main-thread-switch-and-scroll-lag: switching Main to thread and scrolling stay off the layout path'
status: open
priority: 0
issue_type: bug
labels:
- vibe-talk
- ui
- performance
created_at: 2026-11-19T17:24:25-05:00
updated_at: 2026-11-19T17:24:25-05:00
---

# Description

[watermelon-full-preview] Owner report (thread 225, 2026-11-19, about 2:35 PM ET, verbatim): "lag on Main to thread and scrolling". #1913 vibe-talk-search-typing-lag (landed 397ea3f5) fixed search typing; its handoff records the remainder this issue owns: switching Main -> thread costs about 700 ms in one long task with about 900 synchronous layout reads, and in the owner's Chrome trace 349 ms of layout sits in changeChannelView -> toPosition with footGap, scrollTop and getBoundingClientRect the top self-time. The page holds about 100k DOM nodes and 20.7k listeners on the owner's real channel, and scrolling also lags.

Goal: switching between Main and a thread (and back), and scrolling a long channel, stay responsive. The switch does a bounded, O(1) amount of synchronous layout work however many rows the channel holds, and the reader's place is kept exactly.

# Design

- Measure first on a seeded local server of realistic size (at least 2000 rows, threads with replies; never the live instance): per-action counts of synchronous layout reads (getBoundingClientRect and friends) and long tasks for Main -> thread, thread -> Main, and scrolling, in the style of #1913's search_typing_fast_browser.py gate. Find the blocking work; do not guess.
- Fix Main -> thread first, then scrolling: avoid forced synchronous layout in changeChannelView/toPosition — batch reads before writes, measure only the needed neighbourhood, cache row heights. Cut DOM size or listeners where cheap: event delegation instead of per-row listeners, content-visibility:auto for off-screen rows. Full virtualisation only if the bounded fix cannot hold, and only after telling coordb (it is a big change).
- A gate check (a new dag.d fragment, next free number 168) that bounds layout reads per Main -> thread switch independently of row count, on phone and desktop, and asserts the reader's place is kept.
- Evidence: before and after numbers per action, plus shots showing the thread view and scroll position unchanged, phone and desktop, light and dark.
- Dynamic testing (owner thread 185): exercise the owner's exact sequences on a test instance through the real server router (stub only external upstreams), and attach evidence; a page test that injects a fake client call does not count. Screenshot matrix on a test instance (light and dark, keyboard open with a really shrunk viewport and closed, phone 412x915 and desktop 1280x800, Discuss half-pane open in at least the phone shots, plus the targeted before/after states), from a fresh build rebased onto current github/main with BUILD.txt in the harness format, using the shared screenshot harness. Page tests at phone and desktop sizes for the exact defect, condition-based waits only (owner rule 346).

# Acceptance Criteria

- Before/after numbers (measurement, not gating) on the big fixture: synchronous layout reads and long tasks per Main -> thread switch, per thread -> Main switch, and per scroll, on phone and desktop profiles; after the fix the switch does no O(rows) synchronous layout work.
- The gate (dag.d 168) bounds layout reads per Main -> thread switch independently of row count on phone and desktop, and asserts the reader's place is kept (same anchor message at the same offset around the switch).
- Switching Main -> thread -> Main shows the thread view correctly and keeps the reader's place; scrolling a long channel stays responsive with no per-row synchronous layout loop.
- Existing view-switch, thread and reading checks stay green; no new per-switch or per-scroll layout read loop is introduced.
- Selected validation passes locally before the owner-tier gate; the gate passes or pre-existing reds are proven on a clean github/main checkout.

# Notes

Owner-priority, fast-track: deployed on landing. Lane mu-threadlag/1920. Nested worktree .muse/worktrees/threadlag on branch mu-threadlag/1920. Related: #1913 vibe-talk-search-typing-lag (search typing; this issue is its recorded remainder), #220 view-switch-instant. New vibe-talk checks go in tests/dag.d/<next-free-NNN>-<job>.yaml; numbers are never reused (next free at filing: 168).
