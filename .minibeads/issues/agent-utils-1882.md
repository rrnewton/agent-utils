---
title: 'agentctl-false-failed-deliveries: delivered prompts are filed failed when screen verification misses them'
status: open
priority: 2
issue_type: bug
labels:
- agentctl
created_at: 2026-11-18T00:00:00.000000000+00:00
updated_at: 2026-11-18T00:00:00.000000000+00:00
---

# Description

A read-only audit on one host (2026-11-18) found 25 entries across 12 lanes in agentctl's FAILED queue, 7-43 h old, and all 25 had actually been delivered: each lane's own transcript holds the exact text as an accepted user turn within 15 min of queueing (Claude Code prompts and mid-turn queued commands, Codex /goal updates that took effect, a Muse /compact that ran). One Muse lane had 8 marked failed; all 8 appear in its session journal as accepted turns.

The failure modes are all screen-verification misses after the harness took the prompt: the pasted prompt was not observed as the whole editor within the window, it left the composer without visible submission evidence, the peer pane could not be re-read after submission, or the delivery was cancelled after pasting. #1099 claude-stranded-false-lost fixed one such path (a stranded Claude Code paste proved from its transcript); the fresh-delivery and other-harness paths still file delivered prompts as failed (`possibly_submitted`).

Harm: stuck scans flag the entries for redelivery, and redelivering creates duplicates (three audited texts had already arrived twice).

# Design

Before a queue drain files a delivery as failed, confirm against the harness's own transcript/session file where agentctl has an adapter (Claude Code, Codex, Muse): a prompt found there as an accepted user turn, written at or after the delivery's claim time (less a short slack for the paste that preceded it), is delivered and is recorded processed/queued, never failed. The check is a new `HarnessAdapter` transcript proof beside the existing per-harness transcript readers, reusing each adapter's session-pinned transcript location, so another session's identical text never counts. Screen evidence stays primary; the transcript is an extra positive proof consulted only when the screen path is about to give up, and a prompt absent from both is still filed failed as before. No prompt is ever retyped by this check.

# Acceptance Criteria

- A delivery whose screen verification fails but whose prompt the harness transcript records as an accepted user turn at or after claim time is recorded delivered (processed/queued), not filed in `failed/`, for Claude Code, Codex and Muse.
- A prompt absent from the transcript is still filed failed exactly as before, and an identical entry from before the claim (or in another session's transcript) does not rescue it.
- Fake-transcript tests cover each harness (accepted shapes prove delivery; older, prefix, tool-result and assistant-echo shapes do not), plus a drain-level test per outcome.
- The fix is exercised manually against test agents through the real CLI before landing.
- `common/docs/agentctl/harness-support-matrix.md` is updated in the same change.

# Notes

Related closed precedent: #1099 claude-stranded-false-lost (stranded Claude Code pastes proved from the session transcript), #1085 muse-send-staged-unsubmitted, #1413 revive-keeps-queued-messages. Audit evidence was read-only and per-lane; no lane, host or person is named here.
