---
title: 'agentctl-busy-claude-false-possibly-submitted: sends to a busy Claude Code pane are retained as failed although the prompt was delivered'
status: open
priority: 2
issue_type: bug
created_at: 2026-11-06T19:08:27.000000000+00:00
updated_at: 2026-11-06T19:08:27.000000000+00:00
---

# Description

On agentctl d9234603 and 15345285, three sends on 2026-11-06 to a Claude Code coordinator pane that was in a long turn returned outcome `possibly_submitted`, and each was retained as failed. The pane was running background shells and subagents. Every prompt was in fact delivered: it shows in the transcript, the composer is empty, and the agent acted on it.

The screen at that moment showed a running turn (spinner, a "running PreToolUse hooks" line), with the delivered prompt already in the transcript above. a7cd372f's queued-message evidence evidently doesn't recognise that state as delivered.

A second Claude Code coordinator pane saw the same symptom the same day: failed-digest notices for at least three messages that had reached it and been acted on. In one of its readback entries, a receipt was present but the result was `not-seen`. Each false failure later shows up as a "may not have reached you" digest typed into the busy pane, which costs a turn and can strand text in the composer.

Message ids from the first pane, for its readback.jsonl: 01793989429469075785-1922319, 01793991485109483407-3170143, 01793992030829478623-1437407.

# Design

- Capture the busy-turn screens (spinner, a hooks line, the prompt in the transcript, an empty composer) as fixtures from the readback evidence.
- Treat "the prompt text appears in the transcript above the current turn and the composer is empty" as delivered, even while a turn is running. Keep `possibly_submitted` for genuinely ambiguous screens.
- Where a receipt is recorded but the read-back says `not-seen`, decide which wins, and say so in the record.

# Acceptance Criteria

- Fixture tests: each of the three captured busy screens resolves to delivered, not failed.
- A screen where the prompt is still in the composer is still retained or retried, not marked delivered.
- No failed-digest is produced for a delivered prompt in these cases.

# Notes

Reported 2026-11-06 by the lead coordinator.
