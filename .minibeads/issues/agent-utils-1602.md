---
title: 'codex-capacity-live-screen: classify the live capacity screen with its background-terminal line and composer placeholder'
status: open
priority: 2
issue_type: bug
external_ref: gh-293
created_at: 2026-11-15T17:47:04.000000000+00:00
updated_at: 2026-11-15T17:47:04.000000000+00:00
---

# Description

A live Codex model-capacity stall was classified idle-no-verdict and never recovered, although the agent was opted into auto-recovery. The live screen differs from the synthetic fixture in two ways: a background-terminal status line sits between the capacity error row and the composer, and the composer shows its placeholder text. Reading classify() in rs/agentctl/src/stuck.rs (unchanged since the pre-report classifier landed): any user-turn row clears the accumulated error text, and the placeholder row qualifies as a user turn (non-empty, non-digit text after the prompt glyph), unlike the fixture's bare prompt row. The capacity error is therefore cleared before classify_error runs; at best the stalled-goal footer yields the non-recoverable goal-stalled cause.

Live screen from the report (use verbatim as the fixture):

    ■ Selected model is at capacity. Please try a different model.
      1 background terminal running · /ps to view · /stop to close
    › Ask Codex to do anything
      GPT-6.1-Sol medium · ... · Goal stalled (/goal resume)

# Design

Add the live capture as a classifier fixture. An empty composer (placeholder-only prompt row) must not clear a preceding anchored error, matching the documented rule that an empty composer does not clear prior errors. Keep the guards that ordinary prose, quoted history, and tool-output copies mentioning these phrases are not faults.

# Acceptance Criteria

- The live-screen fixture classifies as model-capacity (recoverable), and watch recovery applies to an opted-in agent showing it.
- Prose/tool-output mutation fixtures still classify as no fault.
- A real new user turn after the error still clears it.

# Notes

[watermelon-full-preview] Filed from GitHub #293 codex-capacity-live-screen (https://github.com/rrnewton/agent-utils/issues/293) during Request 098 triage. The placeholder-clears-error mechanism was read from classify(); the acceptance fixture is the proof. Builds on #585 agent-watch-recovery (closed).
