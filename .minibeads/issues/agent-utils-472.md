---
title: 'gate-flake-hardening: make the browser checks and unit budgets hold under host load'
status: open
priority: 1
issue_type: task
labels:
- vibe-talk
- validation
created_at: 2026-10-23T21:30:11.219586130+00:00
updated_at: 2026-10-23T21:30:11.219586130+00:00
---

# Description

[opus 5.5] vibe-talk gates (~40 min each) keep failing on checks that pass when re-run alone, under host load (load average up to ~99 with 4-5 gates at once). Observed 2026-10-23: read-aloud-blocks-browser lit sentences 2-3 twice; scroll-stability-browser phone-412 'Cmd+3 from Main scrolled up moved the reader'; thread-card-header-browser 'the card at the top jumped, through only [276]' (also on pristine main); screenshots harness timeouts (54-help-sections, 12-collapsed-long-transcript); vibe-talk.test.unit at its CPU budget, with node tests/js/voice_page.test.mjs burning 450-530 s of CPU inside it. Browser steps run in a 1-core cpu.max box (dagrun's default for undeclared widths), so a burst of renderer work is throttled and frames drop.

# Acceptance Criteria

Each flaky check made event- or state-driven without weakening it, each with a negative control that still fails on the bug it guards; pass rates under a reproducible load before and after; the page suite in its own dagrun step with its own CPU budget, selection rules kept correct; gate wall time measured before and after.
