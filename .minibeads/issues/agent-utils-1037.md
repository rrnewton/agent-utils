---
title: 'shots-port-collision: the screenshot steps use fixed ports, so two concurrent gates fail each other'
status: closed
priority: 1
issue_type: bug
assignee: cl-start
created_at: 2026-11-08T01:10:00.000000000+00:00
updated_at: 2026-11-08T04:10:00.000000000+00:00
closed_at: 2026-11-08T04:10:00.000000000+00:00
---

# Description

vibe-talk/tests.dag.yaml runs the screenshot harness on fixed ports: vibe-talk.shots.screenshots-phones on 18091, screenshots-desks on 18291 and the 1920 desk profile on 18491 (VIBE_TALK_SHOTS_PORT=...). gate.py admits up to two shared gates at once, and each gate is its own dagrun, so dagrun's resource caps cannot serialize the two. When two page gates reach their screenshot steps together, the second fails at once:

    vibe-talk run.sh: something is already listening on port 18291.

On 2026-11-07 a full gate failed exactly this way (screenshots-desks and screenshots-phones, both 0 s, exit 1) while another page gate ran. It reads as a failure of the change under test and costs a rerun.

# Design

- Have run.sh --screenshots pick a free port itself (bind port 0, or probe from a base) when VIBE_TALK_SHOTS_PORT is unset or "auto". Pass the chosen port to the browser harness, and print it.
- Drop the fixed ports from tests.dag.yaml (or set them to auto). Keep the refusal when an explicitly requested port is busy.
- Check the other steps that start a server for fixed ports (browser checks, live-relay checks) and give them the same treatment.

# Acceptance Criteria

- Two copies of the screenshot steps run at the same time from two checkouts both pass.
- No step in the validation graph hard-codes a listening port unless it is declared exclusive.

# Notes

Found while gating #624 work on 2026-11-07, the first evening with two concurrent page gates.

Raised to P1 on 2026-11-07: the same collision cost reruns on both hosts the same day. Add a test that runs two screenshot shards at once.

## Resolution

Landed as dfc9326e "vibe-talk: screenshot runs claim their ports, so concurrent gates never collide" (cl-start, gated as 1b853ebb: full gate validate: OK, 152/152).

- Cause of the fixed ports: run.sh's automatic pick probed and released ports before the server bound them, and it preferred 18091, so shards that started together all chose 18091. The graph pinned 18091/18291/18491 to avoid that, and pinned ports collide across two gates.
- Fix: run.sh --screenshots claims each of its three ports (server, +1, +2) with a flock on ${XDG_RUNTIME_DIR:-/tmp}/vibe-talk-shots-ports/<port>.lock. The claims are held on fds 7-9 for the whole run and inherited by the server, mock and browser. A port the caller gives is claimed too, and refused by name when another run holds it. tests.dag.yaml names no ports.
- Acceptance:
  - Two dagruns of all three shard steps started together on one host (six screenshot runs at once) both passed.
  - The new graph contract test_no_step_listens_on_a_fixed_port_unless_it_runs_alone forbids fixed listening ports outside gate-exclusive steps; it fails on the old graph.
  - test-run-sh.sh checks that a claimed-but-unbound port is passed over and that an explicit claimed port is refused.
- Riding in the same stack: ed4aa7b9 (gated as 832c5dd8), the #998 gate-browser-steps-under-load desktop-layout width diagnostic.

