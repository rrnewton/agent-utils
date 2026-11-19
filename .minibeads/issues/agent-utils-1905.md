---
title: 'agentctl-narrow-pane-stranded-paste-other-sender: stranded prompts from other senders are not recovered'
status: closed
priority: 1
issue_type: bug
labels:
- agentctl
created_at: 2026-11-19T00:00:00.000000000+00:00
updated_at: 2026-11-19T00:00:00.000000000+00:00
---

# Description

Follow-up to #1897 agentctl-claude-narrow-pane-typed-not-submitted
(landed b9f5dc99). On a build including that fix, three Claude lanes
in 42-column, about 80-row panes, each with agent status done, held an
agentctl-typed prompt stranded in the composer: worker-to-lieutenant
and lieutenant-to-worker messages (senders other than the coordinator
whose deliveries were queued behind them; 3–5 messages queued behind
each). Every `agentctl drain` refused with "does not show a
recognisable claude composer after 15s; nothing was typed". One manual
Enter per pane submitted the stranded text and the drains then
delivered.

The recipient queues do record those prompts as typed. The recovery
match sets, however, key a stranded record by the wrong clock and
bound, so on busy lanes the stranded prompt falls out of the set the
recovery compares against:

- The lower-region/backstop candidate reader
  (`paste_backstop::delivered_prompts`) times a `processed/` or
  `failed/` record by its original `inflight_at` only and keeps only
  the newest 20 candidates: a record a pass left stranded
  (`stranded_at` set, `possibly_submitted`, or a `late_paste_lost`
  error) is excluded once its typing is older than the lookback, and
  evicted by newer deliveries on a busy lane, even though it was left
  stranded recently.
- The composer-shaped matcher (`stale_draft::find`) vouching for a
  whole-prompt draft under `Unproven` counts only a record in
  `failed/` as not proved delivered; a stranded record (in its inbox,
  or moved to `processed/` as possibly submitted) is likewise not
  proved delivered — agentctl gave up on its key — but does not vouch,
  and only the latest five deliveries are searched for a text draft.

# Design

- `delivered_prompts`: a record carrying `stranded_at`, marked
  `possibly_submitted`, or holding a `late_paste_lost` error is timed
  by the later of its typing and its stranding, and is never evicted
  by the newest-20 cap (the lookback still bounds it); other
  candidates keep today's timing and cap. Sender is never a filter:
  the recipient's queue is the only match set.
- `stale_draft::find`: a stranded-marked record vouches like a
  quarantined one for a whole-text or character-count draft (it was
  never proved delivered, and a history recall cannot produce a
  prompt the agent never took), and stranded-marked records are
  searched beyond the latest-five window, within the existing text
  lookback. Strict per-registry attribution (#1683) is unchanged —
  the stranded records in evidence are in the recipient's own queue —
  and a foreign or human draft still gets no key.
- Recovered records are noted recovered exactly as today
  (`draft_recovered_at` / evidence), so the drain marks them delivered
  and never types them again.

# Acceptance Criteria

- A stranded prompt from another sender, recorded in `processed/` as
  possibly submitted (or `late_paste_lost` in `failed/`) with an old
  typing time and a recent stranding, is recognised and submitted
  with one verified Enter, at pane widths 30, 42 and 80, and its
  record is noted recovered.
- Newer deliveries on a busy lane do not evict that record from the
  recovery match set.
- The same holds where a composer reader does parse the screen.
- A foreign draft at the same widths still gets no key, and no prompt
  is ever typed twice.
- Tests use condition-based waits (poll counts / fake clocks) only.

# Notes

Related: #1897 agentctl-claude-narrow-pane-typed-not-submitted, #1683
pane-lock-held-by-stuck-sender, #1560
harness-agnostic-stuck-paste-backstop, #884, #720. Evidence read
read-only from the affected registry (queue records of the three
lanes); no lane, host or person is named here.

## Resolution

[watermelon-full-preview] Landed 522f4b4c on github/main. The stranded
records were all in the recipients' own queues (not cross-registry, so
#1683's per-registry attribution is unchanged); the match sets simply
stopped considering them. `delivered_prompts` now times a record
carrying `stranded_at`, marked `possibly_submitted`, or holding a
`late_paste_lost` error by the later of its typing and its stranding,
and such records are never evicted by the newest-20 cap (other
candidates keep it), and `stale_draft::find` treats a stranded record
as not proved delivered for vouching and searches stranded records
beyond the latest five deliveries, within the existing lookbacks.
Sender is never a filter. Tests cover a processed possibly-submitted
record from another sender recovered at widths 30, 42 and 80 with its
record noted recovered, the same beyond the latest five where a
composer parses, and the candidate cap; a foreign draft still gets no
key and nothing is retyped. Gate: owner-priority full gate had one
red step untouched by this change (`repository.check.wrkslots-debug-test`,
CPU timeout while the agentctl suite passed); its single solo rerun
passed and land-main accepted it naming that step. No persisted queue
or record field changed. Live deployment is the coordinator's batch.
