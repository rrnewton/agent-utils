---
title: 'wrkslots-eden-arm-recipe: a tested recipe for wrkslots Eden slots on an ARM host'
status: closed
priority: 2
issue_type: task
labels:
- wrkslots
created_at: 2026-11-15T17:00:00.000000000+00:00
updated_at: 2026-11-16T02:30:00.000000000+00:00
---

# Description

Deliver a tested recipe for running wrkslots Eden slots on an ARM
reservation host, plus any agent-utils fix the recipe needs. The
recipe answers four asks from evidence gathered on that host:
registering a slot with `wrkslots create --eden` (exact commands,
config and outputs); where the clones live and the disk floor;
retirement that runs #1204's salvage and remove checks first and
never deletes evidence or another lane's checkout; and sl/jf
authentication, tested, with the exact commands. It links
#1204 wrkslots-eden-checkouts.

# Design

- Generic recipe as settled user docs in
  `common/docs/wrkslots/USER_GUIDE.md` (an "Eden slots" section),
  with placeholders only; the host-specific runbook lives outside
  this repository.
- Fixes the recipe needed, each with a focused test in
  `py/wrkslots/tests/test_eden.py`: the Eden checkout path is always
  `<slot>/checkout`; audit and slot-contents checks accept the Eden
  slot layout; the process censuses exempt the slot's own Eden
  daemon, privhelper, log helpers and mount; Git salvage and remote
  checks skip Eden checkouts; the synthesized removal handoff names
  Eden salvage evidence; `sl diff` salvage uses valid Sapling
  syntax; `/proc` parsing accepts boot-time start ticks, ESRCH grep
  diagnostics and kernel threads; validation-run evidence exempts
  the slot's own daemon unit and mount.
- #1423 eden-daemon-containment stays open and unassigned; the
  recipe notes it where it matters.

# Acceptance Criteria

- On the ARM host, with its own isolated project and private
  `--config-dir`: a validate Eden slot creates, mounts, reports
  under the cap and removes; an agent Eden slot commits work and
  `wrkslots remove` salvages it (verified bundle under the control
  directory) before removing the slot and stopping its daemon.
- The generic recipe covers registration, clone location and disk
  floor, salvage-first retirement, and authentication, with no
  host or consumer names.
- The wrkslots tests for the touched area pass; strict mypy on the
  changed Python files is clean.

# Notes

Findings from the host trial: `wrkslots init` writes a project
command symlink whose target does not resolve (invoke the
interpreter entry point directly); the default `eden_host_cap` of
3 refuses on hosts whose default daemon already holds 4 outer
checkouts — raise it only in your own project's configuration.
Daemon containment remains #1423, open and unassigned.

## Resolution

Tested on an ARM host from an isolated project with a tiny local
Sapling backing repo (a large-repo backing was not exercised; the
recipe states what it would need). Validate and agent Eden slots
were created and removed through wrkslots; the agent slot's work
was salvaged to a verified local bundle before removal, and every
private daemon was stopped by the removal. The fixes listed in
Design were required for any of that to complete on a nested-layout
project. Host evidence and exact outputs are recorded in the
coordinator's runbook, outside this repository.
