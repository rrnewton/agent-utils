---
title: 'gate-flake-hardening: make the browser checks and unit budgets hold under host load'
status: closed
priority: 1
issue_type: task
labels:
- vibe-talk
- validation
created_at: 2026-10-23T21:30:11.219586130+00:00
updated_at: 2026-10-23T23:26:25.508315938+00:00
closed_at: 2026-10-23T23:26:25.508315778+00:00
---

# Description

[opus 5.5] vibe-talk gates (~40 min each) keep failing on checks that pass when re-run alone, under host load (load average up to ~99 with 4-5 gates at once). Observed 2026-10-23: read-aloud-blocks-browser lit sentences 2-3 twice; scroll-stability-browser phone-412 'Cmd+3 from Main scrolled up moved the reader'; thread-card-header-browser 'the card at the top jumped, through only [276]' (also on pristine main); screenshots harness timeouts (54-help-sections, 12-collapsed-long-transcript); vibe-talk.test.unit at its CPU budget, with node tests/js/voice_page.test.mjs burning 450-530 s of CPU inside it. Browser steps run in a 1-core cpu.max box (dagrun's default for undeclared widths), so a burst of renderer work is throttled and frames drop.

# Acceptance Criteria

Each flaky check made event- or state-driven without weakening it, each with a negative control that still fails on the bug it guards; pass rates under a reproducible load before and after; the page suite in its own dagrun step with its own CPU budget, selection rules kept correct; gate wall time measured before and after.

# Notes

[opus 5.5] Landed 2e16e459..934f329b: bf09fc30 page suite in its own step (build.tests, test.unit, test.page); 961e63fa read-aloud sentence light ignores underrun silence, the browser check tells boundaries from inside-block gaps and excuses an inside underrun only with a voice-side stall; e95c75d9 card-flight fills backwards, the check slows it through --card-flight after checking the page's own 220 ms; e13e3a04 screenshot waits x3 and expectations re-polled for 5 s; 934f329b scroll-stability holds the thread list from when its own read has landed. Pass rates, before and after (load generators in the handoff): read-aloud-blocks under a process-group freezer 0/5 -> 10/10; thread-card-header pinned to one CPU with three busy loops 6/10 -> 16/16 (and 6/6 on the final code); scroll-stability with the threads read held 0.8 s fails every run -> passes, and 5/5 under the freezer. Gate on the final code: green, 2469.9 s (before: 2431-2556 s today); the browser lane is still the critical path (#474 browser-lane-critical-path). Unverified: how often the screenshot change saves a gate (its failures did not reproduce here); a stall in the browser alone with the voice on time would still fail read-aloud-blocks' inside check. Follow-ups: #474 browser-lane-critical-path, #475 thread-list-first-read-reorder, #476 scroll-older-replies-flake.
