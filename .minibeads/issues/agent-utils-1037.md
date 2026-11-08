---
title: 'shots-port-collision: the screenshot steps use fixed ports, so two concurrent gates fail each other'
status: open
priority: 2
issue_type: bug
assignee: mu-validate
created_at: 2026-11-08T01:10:00.000000000+00:00
updated_at: 2026-11-08T01:10:00.000000000+00:00
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
