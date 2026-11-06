---
title: 'revive-abandon-dead-candidate: an interrupted revive whose candidate died no longer wedges the agent'
status: open
priority: 2
issue_type: bug
assignee: cl-start
created_at: 2026-11-05T17:13:09.823347188+00:00
updated_at: 2026-11-05T17:13:09.823347188+00:00
---

# Description

An interrupted revive whose candidate never came up wedges the agent for good. After a load spike on 2026-11-05, `agentctl revive <name>` for a Codex lane in a consumer team's registry opened a candidate pane, but the harness never started (the pane shows no agent, probably killed under load). Every later revive refuses:

    interrupted revive candidate is not independently verified; refusing another launch

The revive journal (`.agentctl/.revives/<id>/`) still holds the interrupted attempt, and nothing was hand-edited. There is no supported way out. The agent's 4 queued prompts wait behind it.

# Design

- **Resolve a dead candidate from evidence.** A revive that finds an interrupted candidate checks it independently. If the candidate pane is gone, holds no harness process, or its harness process is dead, the attempt is marked failed in the journal, with the evidence recorded. A new launch then proceeds.
- **Automatic only when unambiguous.** When the evidence is unambiguous (the pane is gone, or no process in its tree is a harness), revive does this by itself. When it is ambiguous (for example a live process whose identity cannot be checked), revive refuses as now, but names the explicit way out.
- **Explicit override.** `agentctl revive NAME --abandon-candidate` marks the interrupted candidate failed after the same checks, refusing if a live harness process is in the candidate pane, and then relaunches. Document it in help and the userguide.
- **Queue survives.** Queued prompts are kept and delivered to the revived agent as usual.

# Acceptance Criteria

- Test: an interrupted revive whose candidate pane has been closed. The next revive marks it failed, with the evidence in the journal, and launches.
- Test: the candidate pane is alive with no harness process. The same result, automatically.
- Test: the candidate pane holds a live harness process. Revive and `--abandon-candidate` both refuse, naming the process.
- Test: queued prompts are delivered after the successful revive.
- Live check (through the lead, or the scoped #896 route while it exists): the wedged consumer-team record revives, with its 4 queued prompts kept.

# Notes

Reported by the lead on 2026-11-05 at 12:12 PM ET, from a consumer team's coordinator (P2, ranked after #896). The wedged record and its journal are kept as evidence in that team's registry on this host. They are read-only for us; ask through the lead for any test against them. Assigned to cl-start after #896 and #897.

## Progress (cl-start, 2026-11-06)

[opus 5.5] Implemented in `revive`. Before finishing a revive interrupted while launching (journal phase `launching`, nothing published, the old generation still the active record), agentctl checks the staged candidate on its own:
- **Live:** its pinned harness process is alive, or Herdr reports an agent in its pane. The revive verifies and publishes it as before.
- **Dead:** its pane is gone, or the pane holds only an idle shell with no child process (the same idle-shell proof revive uses for stale panes). The attempt is marked failed automatically, and the same command launches again.
- **Unproven:** anything else, including a candidate that recorded no pane. A harness started before the pane was recorded cannot be checked, so the launch-once latch stands. The refusal now names `agentctl revive NAME --abandon-candidate`, which marks the attempt failed and relaunches. It is refused while the candidate is live, naming the process or pane.

Where the evidence goes:
- Marking failed moves the journal and the operation directory, whole, into `REGISTRY/.revives/.abandoned/OLDTOKEN-SECONDS/`.
- Beside them, `abandoned.json` records the name, both tokens, the phase, the evidence and `explicit`.
- The revive result carries the same record as `abandoned_candidate`.
- The `.abandoned` directory's leading dot keeps it out of both editions' journal scans.
- `--dry-run` shows the plan: `revive` with the evidence, or `blocked`.

Restore revives through the same code, so it abandons a proved-dead candidate too.

Tests:
- A candidate pane closed, and one left with an idle shell: auto-abandoned with evidence, then launched.
- A live candidate: `--abandon-candidate` and its dry run both refuse, and nothing changes.
- No pane recorded: refused, naming the flag; the flag relaunches it; the old queue is archived exactly as in any revive.
- `--abandon-candidate` with nothing interrupted is refused.

Queue criterion answered (coordinator and lead, 2026-11-06): **archived, not replayed, and reported to senders**. Replaying a stale queue after a dead candidate risks duplicate or out-of-date prompts. Every revive, abandoned candidate or not, now does the following:
- It archives the old queue as before.
- It names each undelivered prompt (inbox, in flight or failed) in the result's `archived_prompts`, with its ID, sender, first line and lane.
- It records each one in the new generation's `failed/`, with the reason `archived by revive, not delivered` and the archived path.
- So a sender asking after it reads failed, never delivered, and the failed-queue digest names it to the revived agent. Nothing types it.
- The test checks the result row, `message_state` reading Failed, the digest entry, and that nothing was typed.
- Sender side:
  - A chat bridge asks `message_state` and reads Failed (tested in agentctl).
  - An agent-teams team message handed to that queue settles from `accepted` to `possibly_submitted` when its receipt is read. The ledger's queue path is the agent's queue, which the revived generation now holds.
  - Because of the existing #904 notice, the sender is told it may not have arrived. Its detail is now the revive's reason (`archived by revive, not delivered ...`) instead of the generic quarantine text.
  - Tested in agent-teams' model harness. The state stays `possibly_submitted` because the ledger has no "not delivered" terminal state; adding one would be separate ledger work.

Opt-in replay is #925 revive-replay-queue (P3, unassigned).

Remaining: the live check on the wedged consumer record, through the lead.
