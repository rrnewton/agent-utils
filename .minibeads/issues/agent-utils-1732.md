---
title: 'agentctl-muse-midturn-typed-not-submitted: a mid-turn Muse delivery leaves typed text unsubmitted and the watcher calls it foreign'
status: closed
priority: 1
issue_type: bug
labels:
- agentctl
created_at: 2026-11-16T19:10:00.000000000+00:00
updated_at: 2026-11-17T05:02:00.000000000+00:00
closed_at: 2026-11-17T05:02:00.000000000+00:00
---

# Description

Owner rule (owner 6539): every stuck composer is an agent-utils bug.
Report (2026-11-16, 2:03 PM ET): twice in one day agentctl typed its own
message into a Muse lane's editor mid-turn without submitting it;
watch-agents then reported the text as a draft agentctl did not type.
Pressing Enter by hand fixed both.

Reproduced live on Muse 1.4.23 in a throwaway pane (2026-11-16): text
typed into the editor followed at once by Enter can leave the prompt
sitting unsubmitted, idle and mid-turn alike; a later Enter submits it
(mid-turn it steers, printing the prompt as a user turn). Muse drops a
submit key that lands while it is still taking in the paste or
redrawing, most easily while a turn streams tool output.

Why agentctl does not recover it today:

- The Claude Code / Codex path presses a second Enter about 1 s after
  every delivery (#1691 agentctl-second-enter, `second_enter` in
  `rs/agentctl/src/submission.rs`), but Muse never runs
  `submit_verified`: its busy path (`muse_followup_submit` in
  `rs/agentctl/src/subagents.rs`) retries Enter only while the editor
  shows the exact pasted prompt, and its idle path presses Enter once
  and re-presses only inside the working-confirmation wait
  (`MUSE_IDLE_PRESSES`). A paste Muse draws late, wrapped, or as a
  placeholder the exact-composer proof does not recognise, or a key
  dropped after the proof window, leaves the text in the editor with
  the delivery already settled as delivered or possibly submitted.
- Attribution then fails, so no backstop submits it. A Muse draft that
  shows only `[Pasted Content N chars]` matches, in
  `rs/agentctl/src/stale_draft.rs`, only the very last prompt typed
  (`deliveries = 1` for `Draft::Placeholder`); a text draft matches
  only the last few deliveries. On a busy lane the stuck paste is
  routinely no longer the latest, so `sweep_editor_drafts` in
  `rs/agentctl/src/agent_watch.rs` calls agentctl's own text
  unattributed — "a draft agentctl did not type" — and never flushes
  it. The #1691 size attribution covers only an editor no reader can
  parse (`placeholder_matches_stranded`), not a readable Muse editor
  holding a placeholder.

# Design

- Busy Muse delivery never leaves text in the editor: while the
  editor still holds the pasted prompt in any form the delivery can
  recognise (exact text, wrapped text, or its paste placeholder), the
  submission key is pressed again on the existing doubling schedule;
  a prompt Muse holds in its `◆ Queued input` block counts as queued
  (submitted at turn end), never as still staged. A delivery that
  cannot prove Muse took the prompt takes it back out of the editor
  (the existing withdraw) or reports it not staged; it never settles
  while its own text still sits in the editor unrecognised.
- After a Muse delivery settles, the same delayed re-check the
  Claude/Codex path runs (#1683) applies where the runtime enables
  it: if the editor again holds exactly the prompt just sent, one
  Enter; nothing on foreign text or a blocked screen.
- Placeholder attribution by size, as #1691 commit 2 began: a Muse
  `[Pasted Content N chars]` draft matches any typed record in its
  lookback whose paste length is N (Muse's count, tabs expanded),
  not only the latest delivery, in the watcher sweep, the delivery
  recovery, and the stale-draft flush alike. Text drafts keep their
  existing vouching.
- Both editions where the tool has both: Muse follow-up delivery and
  the watcher are Rust-only (the Python edition waits for an idle
  Muse editor), so the fix is Rust-only with the parity note kept in
  the harness support matrix, whose Muse cells are updated in the
  same commit.

# Acceptance Criteria

- Fake Muse screen, busy turn: Enter dropped while the paste shows
  only as its placeholder, then accepted — the delivery is proved
  queued or steered, the editor ends empty, exactly one paste typed.
- Fake Muse screen, busy turn, then idle: a prompt Muse returns to
  the editor after the delivery settled is submitted by the delayed
  re-check or by the next delivery's recovery, never left sitting.
- Watcher sweep: a readable Muse editor holding a placeholder whose
  size matches an older (not latest) typed delivery is attributed to
  agentctl and flushed after the sweep age; foreign text of the same
  shape is never pressed.
- Tests use poll counts and fake clocks, no wall-clock budgets
  (owner rule 346).
- `make validate` passes on the gated HEAD.

# Notes

Related: #505 agentctl-muse-followup, #678 muse-bridge-latency,
#697 send-muse-while-working, #720 agentctl-muse-stale-draft,
#1085 muse-send-staged-unsubmitted, #1560
harness-agnostic-stuck-paste-backstop, #1683
pane-lock-held-by-stuck-sender, #1690 pane-queue-linearize
(cross-registry ordering; out of scope here), #1691
agentctl-second-enter.

## Resolution

Landed c3ff6fac on github/main (replayed from gated c8864aa7; owner-priority gate pass on base a6729b3b). Unique-size placeholder attribution in `stale_draft::find` and a delayed re-check after every verified Muse steer (`recheck_muse_submitted`), with the harness support matrix updated in the same commit. Five fake-screen tests cover the dropped-Enter placeholder, post-settle return, next-delivery recovery, watcher flush, and shared-size refusal; the live reproduction on Muse 1.4.23 in a throwaway pane is recorded in the issue text. Stacked below #1732, #1750 landed first as 26bd34a7.
