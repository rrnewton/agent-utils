---
title: 'pane-lock-held-on-unobserved-paste: no sender holds the pane lock while waiting for its paste'
status: open
priority: 0
issue_type: bug
assignee: cl-start
created_at: 2026-11-05T11:29:26.054636742+00:00
updated_at: 2026-11-05T11:29:26.054636742+00:00
---

# Description

P0. A coordinator pane on a consumer host had owner messages typed 1.8-6.6 minutes late, in five windows on the morning of 2026-11-05, all on agentctl 47aeba8d.

Each time:
- A sender's `verified` admission ended "pasted prompt was not observed in the claude composer within 300.0s", while it held the pane lock for the full 300 s.
- The next waiter failed with "control command was cancelled".
- The chat bridge logged "cancelled while waiting to lock interactive-agent target" every 10 s.
- Afterwards the stranded paste appeared as "composer already holds unsubmitted text", and draft recovery pressed Enter.

The evidence is the pane's `admissions.jsonl`.

1ddd3c0e zeroed the stage grace in one path (`agent.rs` ~496). The `agentctl send` → queue path still passes `SubmitTimeouts::default()` inside `pane_admission::admit`, so the 60 s + 240 s grace runs under the lock: `subagents.rs` `prompt_guarded` (~1693) and `submit_guarded` (~1729). So does the verified-late re-admit (`agent.rs` ~540).

Two follow-on defects lose or damage messages:
- A waiter cancelled before typing anything is recorded as possibly submitted and lands in `queue/failed`, where it is never retried.
- The stale-composer clear deletes a draft that agentctl itself typed (two cases that morning), instead of submitting it.

# Design

1. `prompt_guarded`, `submit_guarded` and the verified-late re-admit use zero grace inside the lock, return `late_paste`, poll for the paste OUTSIDE the lock, and re-admit to submit, as the `agent.rs` ~496 path does. No sender holds the pane lock for more than a few seconds.
2. "control command was cancelled" before any typing is retryable, not `possibly_submitted`. The request stays queued and is delivered later.
3. The stale-composer clear recognises a draft that agentctl typed (same matcher as draft recovery) and presses Enter on it, rather than deleting it. It deletes only text that is not ours.
4. Find why the paste was not observed. Claude Code may collapse a long paste to "[Pasted text #N +k lines]", or wrap it across 4 lines. Make the matcher accept the paste placeholder as our own paste.
5. Optional: retire a stale delivery-alarm entry from 10-30 (5c8a12ea).

# Acceptance Criteria

- Test: a sender whose paste is never observed releases the pane lock within seconds, and the next waiter delivers.
- Test: a waiter cancelled before typing is retried and delivered, not moved to failed.
- Test: a "[Pasted text #N +k lines]" placeholder is recognised as our paste, both for observation and for the stale-composer clear, which submits it.
- Landed alone; the SHA goes to the lead for an immediate deploy on the consumer host.

# Notes

Reported by the lead on 2026-11-05 at 06:27 AM ET (P0; the owner is affected). Assigned to cl-start, at the top of its queue.
