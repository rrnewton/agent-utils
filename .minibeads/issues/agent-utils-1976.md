---
title: 'vibe-talk-client-perf: standing performance lane — search typing first, then composer and archive'
status: open
priority: 0
issue_type: bug
labels:
- vibe-talk
- performance
- ui
created_at: 2026-11-20T13:45:00-05:00
updated_at: 2026-11-20T13:45:00-05:00
---

# Description

[watermelon-full-preview] Owner requests 247/254/255 (2026-11-20). Verbatim 254: "One thing I'm experiencing pretty consistently is that the individual character typing in search bar is NOT particularly smooth. That would be a good place to focus optimization efforts. I think for a substantial while we're going to need to have a persistent vibe-talk optimization lane focused on having a light and responsive client & server."

Standing lane mu-clientperf (owner's explicit exception to fresh-agent-per-task), client and server. First target is search-bar typing: per-character keypress-to-paint latency on a fixture sized like the owner's cache, at 1x and 4x phone CPU throttling, naming everything that runs synchronously on each keystroke after #1913 vibe-talk-search-typing-lag. Then composer typing and archive/unarchive (`e`), scroll, and `o` fold/unfold. Owner traces of 2026-11-19 and 2026-11-20 are analysed for numbers and function names only; no trace content is quoted or committed.

Phase 1 is measure-and-report only: findings in /tmp/mu-clientperf-report.md and the budget table in ai_docs/reports/measurements/vibe-talk-perf-budget.md, sent to coordb; no fix lands without coordb's go per fix.

# Design

- Never call the live server. Local server / page harness built from aee56a3c (the deployed SHA), synthetic fixture scaled from counts only of a read-only copy of the owner's cache (15,041 messages, 1,245 threads, per-thread p50 4 / p95 39 / max 1,314, 3 composer drafts; no message content in any fixture, test or commit).
- Profile on devbig030 (Chromium/Playwright there) at phone 412x915 and desktop, 1x and 4x CPU throttling: Chrome traces plus event counters per keystroke/action (layout reads, class/DOM mutations, storage reads/writes, requests, long tasks) — counts gate, wall-clock numbers live only in the report (owner rule 346).
- Each later fix: one issue (number from coordb), one commit, before/after under the same harness, coordb go before each fix, owner-UI fast-track, screenshots reviewed before deploy.
- Server side after the first client fixes: profile the endpoints the page calls per keystroke and per action, local server only.

# Acceptance Criteria

- Phase 1 report exists at /tmp/mu-clientperf-report.md with search first: per-keystroke latency p50/p95 at 1x and 4x, top self-time functions, long tasks, layout/style counts per keystroke, server round-trips per keystroke, and whether whole-list re-renders happen per input or per archive; composer and archive/unarchive measured the same way; owner-trace findings folded in (numbers/event/function names only).
- The perf-budget table (metric, budget, current p50/p95 at 1x and 4x, last measured SHA, trace path) is current in ai_docs/reports/measurements/vibe-talk-perf-budget.md via coordb.
- No fix is committed before coordb's go on the report; each fix thereafter is one commit with a before/after measurement under the same harness and an event-count gating test (never a wall-time gate).
- Rolling handoff kept current at /tmp/dev-agent-utils-handoffs/mu-clientperf.md after every fix (done, numbers, next 3 targets).

# Notes

Owner-priority, fast-track. Standing lane mu-clientperf under coordb. Builds on #1913 vibe-talk-search-typing-lag (landed 397ea3f5: fast pass from cached folded text, full pass deferred 150ms, bucket chips retire in-event); this lane measures what still runs per keystroke after it. Related: #1899, #1901, #1894, #1895, #1846 windowed lists.
