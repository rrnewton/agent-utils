---
title: 'vibe-talk-windowed-rendering: render only the viewport plus a bounded prefetch margin'
status: open
priority: 0
issue_type: feature
labels:
- vibe-talk
- ui
- scroll
- performance
created_at: 2026-11-18T10:17:59-05:00
updated_at: 2026-11-18T10:40:00-05:00
---

# Description

[watermelon-full-preview] Owner direction (thread 181, 2026-11-18 after the 09:39 ET Android freeze): focus on incrementality and saving CPU. Do not waste compute on things that are off screen. Only a small bit is visible at a time; rendering and updating must focus there. Off-screen work may happen asynchronously in the background, eagerly up to a bounded point so scrolling stays fast, but never the whole history (5000 messages eagerly hydrated and rendered is insane, especially on a phone). Prefetch at a set amount. Context: each live refresh rebuilding all ~300 rows caused the freeze; #1839 vibe-talk-composer-stays-in-view-while-dictating found the volatile observed_ms field and its row-reuse fix lands first; this issue goes right behind it. No prior virtualization or windowed-render work exists in .minibeads/issues, git log, or ai_docs (checked 2026-11-18: only unrelated uses of "virtual"/"window" and three IntersectionObserver uses for badges/reader, none for list rendering).

# Design

- APPROVED by coordb 2026-11-18 (design note /tmp/mu-windowed-design-1846.md, this lane). Q1 window numbers approved as starting values (phone 1200 px/side cap 60, desktop 1600 px/side cap 80) as settings-backed constants (core/settings/windowed.ts + DEVICE_SETTINGS renderPrefetchPx/renderRowCap, 0 = per-device default) tunable without code changes. Q2 budgets N=60/80, M=20 approved as hard test assertions. Q3 native find-in-page limitation accepted with in-app find as the answer, stated in WINDOWED_LISTS_HELP; no auto-off on a find session. Q4 rollout S0 (default Off) -> S1 (phone On) -> S2 (separate approval) approved. Q5 step order approved: step 1 incremental cache first (owner thread 182 principle), windowing third.
- Owner thread 186 two-knob rule: (1) DATA reach-back — messages fetched/held as data for local search/filtering, deep on desktop (DATA_REACH_DESKTOP 5000), shallower on phone (DATA_REACH_PHONE 300), at ~1.5 KB/message (~7.5 MB desk / ~0.45 MB phone, byte-capped at 16 MB / 4 MB); (2) RENDER window — DOM rows mounted, small everywhere. Each per-device, overridable in Settings; the desktop data default may shrink later if server search gets fast. Local search/filter runs over the un-rendered cache (filterCachedMessages); the 5000-cache filter-mounts test (<= N rows) lands with step 3's window.
- Step 1 (this landing): incremental cache discipline — per-id upserts by binary search (incremental.ts: upsertMessage/upsertThread/removeMessage, by-id messageIndex, per-entry versions clockOf/versionOf/changedIdsSince), foldLiveIn/foldDeltaIn/mergeIntoCanon/heldCopies rewired to it, no-change copies keep object+array and advance nothing. Builds on #1839's row reuse (315a6e42 gating; rebase when it lands). Steps 2-6 as in the approved note (height index, channel windowing, thread windowing, hydration pacer, a11y + S1).

# Acceptance Criteria

- Design note approved by coordb before code; implementation follows the approved note as a stack of separately landable steps, each gated.
- The CPU-budget tests pass: no-change refresh does zero row DOM work; 5000-message channel mounts at most N rows; one screenful of scrolling mounts at most M rows; off-screen live update does no DOM work.
- Scroll anchoring holds while rows mount/unmount above the viewport (no jump with composer active, drill anchor, or message anchor); height estimation/correction does not visibly shift the viewport.
- Unread counts, search, Read mode, Done/Undo, label chips, jump-to-message/Newest, and find-in-page all work over unrendered rows (degraded behaviour named in the design note where full fidelity is impossible).
- Screen-reader behaviour is specified and tested; the feature can be switched off by setting and reverts to full rendering.
- Screenshot matrix on a test instance (light/dark x keyboard open with real visualViewport shrink/closed x phone 412x915 scale 2.6/desktop 1280x800, Discuss half-pane on phone, before/after) is captured and inspected before landing.

# Notes

[watermelon-full-preview] Owner thread 181 verbatim direction quoted in the Description. The liaison found no prior virtualization/windowed-render work in .minibeads; ai_docs checked 2026-11-18. This lane's design note (sent to coordb 2026-11-18) is the governing document until approved; figures N and M are stated there for coordb approval.
