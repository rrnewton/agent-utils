---
title: 'chat-status-disagreement: owner chat must not wait behind a working/done disagreement'
status: open
priority: 1
issue_type: bug
created_at: 2026-11-02T03:55:00.000000000+00:00
updated_at: 2026-11-02T03:55:00.000000000+00:00
---

# Description

Incident (devbig030, 2026-11-01, 19:30-19:50 ET): a chat bridge
(`agentctl chat run`, a Muse coordinator, adapter `herdr-foreign`)
left 5 owner requests untyped for 2-2.5 h. The bridge log repeated
`request remains pending: pane wJ:pGY did not become idle/done within 0s;
last status=working`, then after a restart `trying again every 10s while
Herdr reports the agent idle or done`. Meanwhile `herdr pane get wJ:pGY`
said `agent_status` `done`, the editor was empty (Muse footer shows
`Goal (paused · 1d 17h 40m)`), but `agentctl status` said `working` for
hours.

Also found: several `agentctl drain`/`agentctl send` processes on old
builds orphaned for 15-20 h, some ignoring SIGTERM, and the coordinator's
queue inbox holding ~36 undelivered agent messages.

Root cause (read-only inspection, 2026-11-02):

- Herdr's `agent_status` is scroll-independent, but agentctl overrides it
  for a Muse pane from the *visible* screen (`visible` read in
  `WorkspaceClient::pane_info`). The pane viewport was scrolled 1052 rows
  up, so the visible rows contained no editor and no footer (0 divider
  lines); `muse_verified_pane_status` fell through to `working` forever.
  The bottom of the pane (`recent-unwrapped`) showed the empty editor all
  along.
- Even at the bottom, the footer `Goal (paused · 1d 17h 40m)` never
  matches the exact `Goal (paused)` notice the paused-goal check looks
  for, so a paused goal with an elapsed duration is not recognised.
- Bridges run `--deliver-while-working`, but Muse typing requires the
  *derived* idle status (the typing baseline refuses any other status),
  so the same screen-proof failure defeated it. The bridge retried every
  10 s without bound.
- The #720 delivery alarm did fire into `delivery-alarm.json` and the
  bridge log, but nothing posts it to the owner or queues it to the
  coordinator, so nobody saw it.
- A drain/send waiting on a lifecycle lock or an idle state had no
  effective bound in the old builds, and a stale lock holder could block
  new deliveries.

Wanted:

1. Status must agree with the pane's bottom when the visible viewport is
   scrolled away from it: classify a Muse pane from the pane bottom
   (empty editor, running-turn row, paused goal including the
   `Goal (paused · <duration>)` form), keeping the pinned-process checks.
   Prove it with a fake-Muse/Herdr test.
2. Owner messages must not wait behind a status disagreement: when Herdr
   reports idle/done and the editor is empty but agentctl's own status
   says working, deliver owner chat requests anyway and log the
   disagreement. Text agentctl did not type is never submitted.
3. Alarm: a chat request undelivered past the threshold must reach the
   owner (bridge post) and the coordinator (watch-agents) with the
   concrete reason.
4. Hung drains/sends: a drain or send waiting on a lifecycle lock or an
   idle state has a bound (time out with a report, release the lock) and
   honors SIGTERM; a stale lock holder from a dead or old process must
   not block new deliveries.
5. Tests for each.

# Notes

## Resolution

Fixed in this change (#734 chat-status-disagreement):

- Muse status is read from the pane's bottom (`recent-unwrapped`) when the
  visible viewport holds no composer frame (scrolled away), in `pane_info`,
  the typing baselines, and the submission waits; the pinned-process checks
  still bracket every read. A paused goal drawn with an elapsed duration
  (`Goal (paused · 1d 17h 40m)`) is recognised as paused. `agentctl status`
  also reports `herdr_agent_status` and `status_disagreement` when Herdr's
  own report differs from the derived status.
- Owner chat follows the screen proof: with the bottom read, a Herdr-`done`
  pane with an empty editor is `idle` and takes the prompt (including under
  `--deliver-while-working`); the bridge logs each status disagreement once
  per change. Text agentctl did not type is never submitted: a staged or
  paused bottom still holds input.
- The owner alarm no longer treats `last status=working` as busy when
  Herdr reports idle/done: it posts the `[chat-bridge WARNING]` with the
  disagreement as the reason. `watch-agents` sees the queue backlog again
  because the turn view also reads the pane bottom.
- SIGTERM/SIGINT now set a termination flag every `SystemRuntime` wait
  polls, so a drain/send waiting on a lifecycle lock or an idle state
  reports (exit 75, prompt pending), releases its locks, and the lock
  timeout names the holder (PID and command) from `lock_wait`. The
  existing 600 s send deadline already bounds lock and readiness waits on
  current builds; the orphaned processes in the incident ran old builds.

Tests: fake-Muse/Herdr cases for a scrolled viewport (status, delivery,
paused-with-duration hold), the paused-duration footer, the owner alarm
under a Herdr-idle disagreement, and a terminated runtime cancelling its
waits.

Correction (coordinator, 2026-11-02): a paused Muse **goal** must not
hold deliveries. Muse accepts and acts on typed input while its goal is
paused (the incident coordinator acted on a typed message at once with
`Goal (paused · 1d 17h 40m)` on its footer, and the bridge delivered to it
all day until the scroll bug). A paused goal is a Muse planning state,
not `agentctl pause NAME`. So `muse_verified_pane_status` no longer
returns `paused` at all: goal-paused + empty editor is `idle` for owner
chat requests and agent messages alike. Only an explicit agentctl pause,
a staged draft agentctl did not type, or a real working turn holds
input. Tests: `a_paused_goal_with_an_empty_editor_takes_an_owner_request`
(Rust and Python), the scrolled-viewport delivery behind a paused goal
with a duration, and the differential's paused frame reading `idle`.

Not done here (reported to the coordinator, read-only in the incident
registry): the ~36 queued messages and the orphaned old-build processes
in the incident registry are untouched; no process was killed and nothing
was typed into the incident coordinator.
