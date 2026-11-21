---
title: 'agentctl-muse-working-delivery: sends to a working Muse lane hang and are filed possibly submitted'
status: open
priority: 1
issue_type: bug
labels:
- agentctl
created_at: 2026-11-20T16:30:00-05:00
updated_at: 2026-11-20T16:30:00-05:00
---

# Description

[watermelon-full-preview] Since about 4 PM ET on 2026-11-20,
`agentctl send NAME [--deliver-while-working]` to a Muse lane that is
working hangs until its timeout kills it (rc 124/143). Failed queue
entries read "pane wJ:pNNN agent-prompt outcome is unknown; prompt may
have been submitted: ... delivery was cancelled after the prompt was
past" and "... the pasted prompt was not observed as the whole". Some
messages arrived, some were lost. Without `--deliver-while-working`
the prompt sits in queue/inbox while the lane has a Backgrounded task
running (agent status "working" though the terminal host says done),
possibly for hours. Claude lanes deliver fine, and typing the text
into the pane directly works.

Candidate causes, to be tested rather than assumed: (1) the Muse
release installed that afternoon changed the composer's paste
rendering while busy (a pasted block collapsed to a placeholder, or a
changed queued-input display), so the paste observation never sees
the whole prompt; (2) host load or slow pane reads expire the
observation window; (3) a recent agentctl change in the
delivery/observation path (the installed global build predates the
symptom by two days, so this needs evidence, not timing alone).

# Design

- Reproduce in a throwaway Muse lane on a scratch registry: idle
  pane, working pane (a long sleep), and a working pane whose only
  activity is a Backgrounded task; compare with an idle pane.
  Capture the pane through the whole delivery so the exact frame
  the observation rejected is on record.
- Name the cause from that evidence: the frame shape the new Muse
  draws while working, the observation window expiring under load,
  or an agentctl regression identified in its own history.
- Fix the Muse delivery/observation path to accept the frame the
  current Muse actually draws while working, without weakening
  the misroute proofs (pinned process, whole-prompt or exact
  placeholder, journal/transcript proof before filing failed).
- Decide, with evidence, whether a working Muse lane whose
  composer is empty and whose only activity is a Backgrounded task
  counts as idle for queue delivery.

# Acceptance Criteria

- A send with `--deliver-while-working` to a busy Muse lane in the
  reproduction completes (verified delivery) instead of hanging to
  its timeout, for both a short prompt and a long one that Muse
  collapses to its paste placeholder.
- A test pins the new Muse rendering the fix accepts, and an
  unlike frame (foreign draft, partial paste, other placeholder
  count) still refuses.
- The named cause is recorded here with its evidence, and the
  backgrounded-task idle question is answered in the resolution.
- A stopgap note reaches the coordinator and affected operators
  if the fix takes more than an hour.
- (Scope addition, separate commit) `agentctl queue retire NAME
  --entry ID|--all-pending --reason TEXT` retires stale pending
  entries so a prompt delivered by hand is never typed later as a
  duplicate: entries move to `cancelled/` with the reason recorded,
  the agent must be registered (an unknown name creates no queue),
  retiring an already-cancelled entry is a no-op, and `--dry-run`
  lists what would be retired without moving anything.
- `common/docs/agentctl/harness-support-matrix.md` is updated in
  the same change if any per-harness support cell changes.

# Notes

[watermelon-full-preview] Cause, found in a scratch lane the same
day: Muse 1.4.25 squeezes its status footer at the 42-column pane
width the lanes use to two fields (`watermelon-full-prev… ·
Launch overrides`, dropping effort and dir; four fields at 80
columns, model + effort at 30, model alone at 20). The footer
recogniser required at least three fields, so no Muse frame at
that width held a recognisable composer: launches failed readiness,
status stuck at working, working sends hung in readiness to their
timeout, and drains that typed filed the prompt possibly submitted
when the paste observation and its take-back met the same
unrecognisable frame. The agentctl build and the three-field rule
both predate the symptom; the frame is stable, so load is not the
cause. Backgrounded-task answer: yes — a lane whose only activity
is a backgrounded shell, with an empty composer and no progress
row, is idle for queue delivery (the terminals panel is not a
footer, #678); the squeezed footer was the only thing reading it
as working, verified live once the fix read the frame.

[watermelon-full-preview] Lane mu-musedeliver/1995, coordinated
by coordb. Related precedent: #1882
agentctl-false-failed-deliveries (transcript proof before filing
failed), #1905 agentctl-narrow-pane-stranded-paste-other-sender,
#1085 muse-send-staged-unsubmitted, #505 agentctl-muse-followup.
No lane, host or person is named here.
