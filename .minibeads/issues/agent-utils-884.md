---
title: 'pane-lock-held-on-unobserved-paste: no sender holds the pane lock while waiting for its paste'
status: open
priority: 0
issue_type: bug
assignee: '014: cl-enter'
created_at: 2026-11-05T11:29:26.054636742+00:00
updated_at: 2026-11-05T11:55:49.000000000+00:00
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
- Item 4, first attempt (the lead, 2026-11-05 1:12 PM ET, P1): a FRESH delivery recognises the collapsed placeholder as its own paste on the first attempt, not only on a stranded retry. The evidence: the placeholder appeared just after our paste, and its `+k lines` matches the prompt's expected line count. Test: a fresh 19-line prompt drawn as "[Pasted text #N +18 lines]" is submitted on the first pass.
- Item 4, Enter under uncertainty (the owner's rule): after the Enter, verify that the composer emptied or the prompt was accepted. If not, press a bare Enter again a bounded number of times (never retyping), and report possibly_submitted only after the bound. Test: a composer that keeps the placeholder after the first Enter gets a second Enter, and the outcome is delivered once it empties.
- Landed alone; the SHA goes to the lead for an immediate deploy on the consumer host.

# Notes

Reported by the lead on 2026-11-05 at 06:27 AM ET (P0; the owner is affected). Assigned to cl-start, at the top of its queue.

## Resolution

[opus 5.5] cl-start, 2026-11-05. Items 1-3 are delivered; item 4 is mitigated, not diagnosed; item 5 is not in this repository. The issue stays open for item 4.

1. No sender waits for its paste under the locks. `prompt_guarded`, `submit_guarded` and the queue drain's verified submission all use `submission::in_lock_timeouts()`: a 10 s stage window and no late-paste grace. A paste that has not shown by then is refused as `stranded_prompt: late_paste`, with what the composer showed. The drain keeps the prompt pending (`stranded_at`, `typed_as`) and returns, releasing the queue lock, the host-wide target lock and the pane admission lock. Later passes press Enter on the paste once it shows, never typing it again. A pass that still sees an empty composer 300 s after the first refusal reports `late_paste_lost`; the drain clears the stranded marks and types the prompt again in the same pass. The old in-lock 60 s + 240 s wait and the verified-late re-admit are gone. A pane admission flock that times out is now `not_staged` ("nothing was typed"), not `unavailable`.
2. A delivery cancelled before anything was typed (`paste_started_at` absent) is `NotStaged`: the prompt stays pending and is tried again, never quarantined as possibly submitted.
3. The chat bridge's stale-composer clear first asks whether agentctl typed the draft, using the same matcher as draft recovery (`submission::recover_typed_draft` over the agent's queue records). If agentctl did, it presses Enter on the draft (`Step::Flushed`): no backup is written and no notice is posted.
4. On a stranded retry, a composer holding a single paste placeholder (`[Pasted text #N +k lines]`, whatever its number or size) counts as our paste and gets Enter only. A fresh delivery still refuses an unknown placeholder. Why the paste went unmatched on the consumer host is still unknown: `admissions.jsonl` holds no screens. The refusal now records the composer evidence, so the next occurrence will show it.
5. The delivery-alarm entry 5c8a12ea is deployment state on the consumer host, not in this repository.

Tests:
- `agent::tests::an_unobserved_paste_releases_the_locks_within_seconds_and_the_next_pass_submits_it`: the submission takes at most 5 s of clock time under the locks, and the prompt is typed once.
- `agent::tests::a_delivery_cancelled_before_typing_stays_pending_and_is_retried`.
- `submission::tests::an_unobserved_paste_under_the_locks_is_reported_stranded_within_seconds`.
- `a_stranded_retry_never_types_again_while_the_paste_may_still_show`.
- `a_stranded_retry_reports_the_paste_lost_after_the_late_paste_window`.
- `a_stranded_retry_submits_its_paste_drawn_as_a_placeholder_of_another_size`.
- `stale_composer::tests::a_draft_agentctl_typed_is_submitted_not_cleared`.

New evidence from the lead, 2026-11-05 at 1:12 PM ET: a consumer team's pane had 5 stranded "[Pasted text #NNNN +18 lines]" composers today (8:50, 8:55, 10:52, 12:45 and 13:07 ET). The 12:45 and 13:04 cases came from senders still on the old 300 s path (a pinned pre-a5bdddf0 agentctl, now being updated). The root problem remains: Claude Code collapses a long paste to a placeholder, so the composer match fails on the first attempt. The two item 4 acceptance criteria above were added for this.

Item 4 moved to the lead team's Claude lane cl-enter (014) on 2026-11-05 at 1:13 PM ET. Items 1-3 were delivered by cl-start. cl-enter rebases onto main after #896 lands before editing the shared delivery code.
