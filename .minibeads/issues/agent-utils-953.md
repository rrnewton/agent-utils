---
title: 'agentctl-busy-claude-false-possibly-submitted: sends to a busy Claude Code pane are retained as failed although the prompt was delivered'
status: closed
assignee: cl-sync
priority: 2
issue_type: bug
created_at: 2026-11-06T19:08:27.000000000+00:00
updated_at: 2026-11-07T01:25:24.447252000+00:00
closed_at: 2026-11-07T01:25:24.447252000+00:00
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

## Resolution

Two paths reported delivered prompts as failed while a Claude Code pane ran a long turn.

- Read-back. A prompt that leaves the composer of a pane already inside a turn gets a receipt marked queued, and settled once the composer stays clear. Read-back ignored that receipt and waited five seconds for the prompt to show; Claude Code draws a queued prompt only when its turn reaches the input, so the row said `not-seen` and the message was kept as failed. Every false failure on both coordinators was such a row (`receipt: true`). Decision: a queued, settled receipt places the prompt in the target as a printed receipt does, so the receipt wins over the missing sighting; the row says `queued`. An unsettled receipt (the prompt may be back in the composer) or one from a pane that ran no turn stays unproven. A `not-seen` row now keeps the receipt's fields and the target's last twelve screen rows, so the next screen that should have proved delivery can become a fixture.
- Late paste. A prompt an earlier pass typed into a busy pane was reported `late_paste_lost` after 300 s although it had arrived. Captured on a live pane: Claude Code draws a prompt it takes at a tool boundary among that tool's result rows, without its header and first rows and cut before its end, so neither its tail nor its whole text ever showed. An unbroken run of 120 characters of the prompt above the composer now counts as taken into the turn.

Tests: three prompts sent in quick succession to a busy Claude screen are delivered with no failed record; an unsettled and a not-queued receipt still quarantine; a queued receipt outweighs an old copy in another pane; a stranded prompt drawn as captured is delivered and never retyped; a shared header alone is not. The Python edition has no queued receipt and is unchanged.
