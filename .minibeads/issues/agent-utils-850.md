---
title: 'unknown-harness-mode: drive a TUI agentctl has no adapter for by harness-neutral screen rules'
status: open
priority: 1
issue_type: feature
assignee: cl-generic
labels:
- agentctl
depends_on:
  agent-utils-715: related
created_at: 2026-11-04T19:44:20.495107320+00:00
updated_at: 2026-11-04T19:44:20.495107320+00:00
---

# Description

[opus 5.5] Owner request (2026-11-04): adopting an ORC minimal TUI was refused as an unknown harness type. Add a best-effort mode (--harness unknown) that uses general-purpose heuristics instead of Claude/Codex/Muse-specific screen text: ready = no menu and a quiet screen; delivery = bracketed paste then Enter; confirmation = our text (fuzzy, letters and digits only) no longer sits at the bottom as it did when staged; recovery = bare Enter presses under uncertainty, bounded and paced, never a second paste; never act on an open menu or dialog. watch-agents for unknown harnesses: idle = screen unchanged; verdicts only from DONE:/BLOCKED: lines. The heuristics are pure shared functions (screen_heuristics) so every harness's admission point can use them as a fallback layer.

# Design

Built on the per-pane admission point being built in mu-flush/paste-flush; a prompt still stranded after the press cap is reported as stranded_prompt for the recovery ladder.

# Notes

[opus 5.5] Left for after mu-flush's pane admission lands (coordb review, 2026-11-04): no empty-composer gate for unknown TUIs (nothing generic says an input is empty, so a human draft would be merged with the paste); an identical earlier prompt still in the last three rows is taken for a stranded copy and submitted with Enter instead of being pasted; the recovery ladder should take over a prompt that stays stranded across passes.
