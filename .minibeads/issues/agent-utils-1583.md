---
title: 'boxed-muse-lane-identity: boxed Muse lanes die at harness start and cannot retire'
status: open
priority: 2
issue_type: task
labels:
- agentctl
- wrkslots
created_at: 2026-11-15T20:00:00+00:00
updated_at: 2026-11-15T20:00:00+00:00
---

# Description

[watermelon-full-preview] A boxed Muse lane cannot start:
`agentctl start <name> --profile muse-... --slot <slot>
--slot-isolation userns` fails at readiness and the record cannot be
retired through the front door (`stop` exits 69, `custom harness
"muse" is not the foreground process`). Live probe 2026-11-15
(throwaway slot and lanes; full table in the #1569 report): the pane
shows the Muse binary print its banner, then `Error setting
effective UID to root: EINVAL: Invalid argument`, and exit. Claude
and Codex die identically under userns (Claude's start even returns
success first: readiness pins the launcher, then the process dies,
and only the slot-owner transfer notices `owner PID is not live`).
The wrkslots userns maps a single uid (`<uid> <uid> 1` in sandbox
`enter_userns`), so no inner uid 0 exists for the harness launcher's
own sandbox entry. Host-side `/proc` observation of boxed processes
works (verified), so identity pinning is collateral: the pinned
process is simply dead. Muse under root isolation is additionally
refused by construction (`RELAY_HARNESSES` covers only claude/codex).
The Eden slot-box entry refusal found alongside (Git sibling rule on
the backing path) is fixed separately under #1569.

# Design

- Primary (mu-boxedlane, in flight): adapter-declared boxed launch
  for Muse: under a userns slot launch, run the runtime beside the
  launcher (`muse.real`) with the harness's own sandbox disabled —
  the slot box is the sandbox — in both Rust and Python agentctl.
  Working set proven live: that binary + disable-sandbox completes
  start/send/read/stop under userns. The userns relay remains the
  fallback if the direct runtime launch cannot be made sound.
- Surface the real error: a harness that dies before readiness must
  fail start with the pane evidence, not `cannot pin custom
  process: No such process` in Rust or a false `running` record in
  Claude's case (readiness re-verify after the composer check; a
  refused slot-owner transfer must fail the start).
- Retire path: `stop` must archive a `launch_failed` (or falsely
  `running`) record whose pinned process is kernel-dead without
  requiring foreground proof (identity-dead proof with the expected
  token, as the dead-native retire already does where reachable).
- Adapter group-leader coverage: claude/muse declare no
  `native_group_leader` (only codex does), so any multi-process
  foreground group fails pinning under userns. Decide per adapter
  and error with the extra foreground processes named, so Claude
  under userns is not the next surprise.

# Acceptance Criteria

- Boxed (userns) Muse, Claude and Codex lanes each complete
  start/readiness/send/read/stop on a throwaway slot, with the exact
  flag sets recorded in Notes.
- A harness dying before readiness fails start naming its pane
  error, and its record retires via stop; regression tests simulate
  the dead-before-ready launch in both implementations.
- Root-isolation Muse is relayed or refused naming supported modes.

# Notes

Probe evidence (2026-11-15, mu-edenimage + mu-boxedlane,
independently reproduced): userns start failures rc75/rc69 with the
EINVAL pane line for all three harnesses; cgroup (limits-only) muse
lane completes the full cycle; the working mount-sandboxed set is
the runtime-beside-launcher launch above. Full matrix in the #1569
boxed-lanes report. Resolution: (fill in at landing)
