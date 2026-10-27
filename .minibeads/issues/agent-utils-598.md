---
title: 'wrkslots-handoffs-subdir: keep handoff records out of the control directory''s top level'
status: open
priority: 2
issue_type: task
labels:
- wrkslots
- layout
depends_on:
  agent-utils-593: related
created_at: 2026-10-27T22:15:22.969685061+00:00
updated_at: 2026-10-27T22:15:22.969685061+00:00
---

# Description

[opus 5.5] Found by the #593 registry-agents-subdir audit (AGENTS.md rule: a growing collection gets its own directory). The wrkslots control directory (the --worktrees-dir, e.g. worktrees/) keeps one HANDOFF.<len>.<machine>.<len>.<slot>.<generation>.json per live slot and one HANDOFF-RETIRED.<...>.json per retired slot beside its fixed items (ACTIVE.<machine>.json, ARCHIVED.<machine>.json, ACTIVE.<machine>.json.lock, ACTIVE.global.lock, EVENTS.<machine>/, slots/, slot-images/). One harness here has 161 HANDOFF-RETIRED files, 3 live HANDOFF files and 9 fixed entries, 173 entries in all; the fixed state files are hard to find. EVENTS.<machine>/ already has its own directory and is fine.

Proposed fix: write handoffs to handoffs/<machine>/<slot>.<generation>.json and retired ones to handoffs/retired/<...>.json (py/wrkslots/cli.py around the HANDOFF path builder, and the Rust wrkslotsd storage and ensure-state checks that whitelist HANDOFF.*). Migration as in agentctl: new writes use the new place at once, readers accept both for 30 days, an explicit migration command (dry run by default), and a dated removal issue.
