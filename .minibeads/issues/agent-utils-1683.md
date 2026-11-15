---
title: 'pane-lock-held-by-stuck-sender: bound the pane lock and recover an unparseable occupied composer'
status: open
priority: 0
issue_type: bug
created_at: 2026-11-15T19:50:00.000000000+00:00
updated_at: 2026-11-15T19:50:00.000000000+00:00
---

# Description

P0. Owner messages to a coordinator pane were stalled for about 107 minutes
(12:56 PM to 2:44 PM ET, 2026-11-15) behind one agentctl notice, and the
first report blamed a sender holding the pane lock with unsubmitted text.

The records show a different mechanism. The notice was enqueued at
12:56:15 PM ET; the drain typed it, did not observe the paste inside the
in-lock stage window, marked the record stranded and released every lock
at 12:56:26 PM ET — the #884 pane-lock-held-on-unobserved-paste design
working as intended. No sender process hung and no flock was held.

What held the pane was the typed text itself, in the composer, invisible
to every recovery that needed a parseable composer:

- Stranded reconciliation never recognised the draft in the composer, so
  no Enter was ever pressed for it. The record ended as `late_paste_lost`,
  possibly submitted.
- Each owner delivery then failed before typing: about ten minutes on
  host target-lock contention, then about 97 minutes on "does not show a
  recognisable composer after 15s; nothing was typed", because the
  occupied composer would not parse.
- The bridge's stale-composer clear (`--stale-composer-clear-after 2m`)
  never started its two-minute clock: the watch only runs the stale step
  when a waiting request's reason carries a composer draft preview, which
  the unparseable-composer refusal never has, and the sweep's own draft
  read needs the same parseable composer. It first observed the draft at
  2:41:29 PM ET, backed it up and cleared it at 2:43:38 PM ET, and the
  owner requests were typed at 2:44:12 PM ET.

#1560 harness-agnostic-stuck-paste-backstop (8b508396) does not bound
this: its watcher backstop presses Enter on a text-visible stuck paste
after three polls spanning 45 s (entries expire after 2 h), it holds no
lock and bounds no lock, placeholder-drawn pastes are out of its scope,
and it was not the edition the watcher ran that day.

# Design

1. One explicit lock-hold bound for a verified submission under the
   pane's locks (typing + submit + verify): the sum of the phase
   deadlines a submission may consume, computed from its `SubmitTimeouts`
   and enforced against the caller's clock in `submit_verified`, so no
   admission holds the pane lock longer, whatever phase stalls. The
   in-lock timeouts the drain uses give the tight bound; the pane
   admission adds at most one pacing interval.
2. Recovery that does not need a parseable composer. When no harness
   composer reader recognises the screen, the pane's visible lower
   region is fuzzy-matched against the prompts the queue records as
   typed (the #1560 normalisation and match rules), in the delivery
   path (`recover_typed_draft`, used by fresh deliveries, stranded
   passes and the stale-composer sweep alike):
   - A draft that matches a typed record is ours: press the submission
     key, prove the text left the lower region, and note the record
     recovered, so the drain marks it delivered rather than lost and
     never types it again. No duplicate.
   - No key is pressed while the screen shows a question, a menu or a
     picker, exactly as on the parseable path.
3. Stale-composer observation that does not need a parseable composer.
   The bridge watch counts a waiting request held by the
   unparseable-composer refusal as composer-blocked, so the stale step
   runs; the sweep falls back to the lower region for its draft read,
   so an occupied but unparseable composer starts the unchanged clock.
   A foreign draft (matching no typed record) is backed up and cleared
   only after the configured stale window, never sooner, and only while
   the agent is not working — unchanged from today. The backup of an
   unparseable draft holds the visible lower-region text.
4. Both editions: the Python `submit_verified` gains the same-prompt
   lower-region flush and the same hold bound for the harnesses it
   verifies (Claude Code and Codex). Any-typed-draft recovery and the
   stale-composer clear remain Rust-only, as the support matrix already
   records for neighbouring capabilities; the matrix's stuck-paste row
   is updated in the same commit, including that #1560 has landed.

# Acceptance Criteria

- Test: a fake sender types a prompt that never shows in a parseable
  composer (it is visible only in the pane's lower region) and its
  submission hangs there; the pane locks are released within the hold
  bound, the prompt is typed exactly once, and a later pass for the
  next (owner) prompt first submits the stranded one from the lower
  region and then delivers its own.
- Test: a draft matching a typed record on an unparseable screen is
  submitted with Enter and noted recovered — never backed up, never
  cleared, never typed again.
- Test: a foreign draft on an unparseable screen starts the
  stale-composer clock, is never cleared before the configured window,
  and after it is backed up (the backup holds the visible text) and
  cleared, with the composer proved empty by the lower region.
- Test: the in-lock hold bound is enforced — a submission whose phases
  would overrun it stops and reports the bound instead of holding the
  pane lock.
- Python edition: the same-prompt lower-region flush and the hold bound
  hold for Claude Code and Codex, with tests beside the existing
  submission tests.
- `common/docs/agentctl/harness-support-matrix.md` updated in the same
  commit.

# Notes

Reported by the owner (request 103, 2026-11-15, 2:50 PM ET: "Messages
stalled. Was it a missing enter key again?"), through the coordinator.
Evidence read read-only on the affected host: the bridge log, the
pane's admissions log, the stranded queue record and its error file,
and the composer's backup file. Related: #884
pane-lock-held-on-unobserved-paste (the in-lock stage bound that worked
here), #720 agentctl-muse-stale-draft (typed-draft recovery), #1546
agentctl-unsubmitted-paste-backstop, #1560
harness-agnostic-stuck-paste-backstop (landed 8b508396; no lock bound),
#1561 harness-support-matrix.
