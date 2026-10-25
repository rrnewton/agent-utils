---
title: 'agentctl-muse-followup: deliver to a busy Muse by steering its running turn instead of waiting for idle'
status: open
priority: 2
issue_type: feature
labels:
- agentctl
- muse
created_at: 2026-10-25T12:36:50.467107347+00:00
updated_at: 2026-10-25T12:36:50.467107347+00:00
---

# Description

[opus 5.5] Owner (2026-10-25): "Why is muse deliver on idle only? Do muse and codex not have the ability to queue in their harnesses" ... "Yes improve that muse integration". The Rust agentctl types into a working pane only for harnesses with a follow-up adapter (Codex by default, Claude Code with --deliver-while-working). Muse had none, so a send waited for an idle editor and holds and stop messages to busy Muse agents arrived hours late. Probed live on Muse 1.4.14 (throwaway sessions only): Muse takes typing while a turn runs; Enter steers the running turn (the prompt is printed at once as a user row above the editor, footer notice "steering the running turn"); Tab queues it for the next turn ("◆ Queued input · ↑ to edit", notice "queued for the next turn"), and a queued prompt waits for the whole turn, including its double-check; Tab does nothing on an idle editor; Esc interrupts the turn even with a draft staged (and submits queued input); ctrl+e ctrl+u plus backspace clear the editor; slash commands run at once even mid-turn; an approval form replaces the editor and Herdr reports blocked; a queued prompt survives an approval.

# Acceptance Criteria

The Rust drain types an ordinary follow-up into a working Muse pane by default, with the same proofs as Codex and Claude: an empty editor first (a draft blocks), Enter under the pinned-process guard, acceptance proved by the prompt appearing as a new user turn above the editor, at most once, a prompt never accepted taken back out of the editor and kept pending, approval forms and panels never treated as a working editor, slash commands still waiting for idle. Fixture tests from captured Muse 1.4.14 screens. The Python edition keeps waiting for idle (owner 2026-10-25: no Python port), recorded as an expected divergence in the cross differential. Userguide and help updated.
