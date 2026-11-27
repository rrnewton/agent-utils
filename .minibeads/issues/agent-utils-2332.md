---
title: 'wrkslots-failed-create-journal-blocks-host-removals: a create that failed before creating anything blocks every later removal while its coordinator lives'
status: open
priority: 2
issue_type: bug
labels:
- wrkslots
created_at: 2026-11-26T19:40:00-05:00
updated_at: 2026-11-26T19:40:00-05:00
---

# Description

On 2026-11-24 a coordinator running under Orc called `wrkslots create` for a lane slot. The create failed before it made anything: `mkfs.ext4` was not on the caller's PATH, so the slot image could not be built. The create journal was left behind with `created: []` and `failure_policy: leave-for-inspection`. Its recorded coordinator is the Orc engine process, which stays alive for days. On 2026-11-26 `wrkslots remove` of a different, unrelated slot refused: "interrupted mutation recorded in ...CREATE...journal; run 'wrkslots recover' first (kind 'create', slot '<other slot>': actor PID ... is live)". `wrkslots status` also showed `RECOVERY REQUIRED`. So one failed create that created nothing blocked every removal on the host for as long as its coordinator lived.

Recovery was possible only from a process descending from that coordinator, because `wrkslots recover --coordinator-pid PID` checks PID against the caller's process ancestry. Neither the refusal nor `wrkslots remove --help` says so. The coordinator had no entry point for `recover` (its tool wrapper exposed create, status and remove only), and an operator shell could not do it either. The way out was a new wrapper subcommand plus a one-off script registration in the live session, which then ran `recover --slot <slot> --abort-create`.

# Design

- A create journal whose `created` list is empty, which is in its leave-for-inspection state after a recorded failure, does not block mutations of other slots: report it in `status` and `doctor` as a failed create to clean up, not as a live interrupted mutation, and let `recover --abort-create` from any coordinator of the project drop it (nothing exists to protect).
- Scope the removal gate to journals that can touch the slot being removed, or to journals whose actor is really mid-operation (heartbeat fresh), not to every journal whose recorded coordinator is alive.
- When a refusal still applies, the remedy names the exact command and the requirement: run it from a process under coordinator PID N (or from that coordinator's tooling), with `--abort-create` for a failed create.

# Acceptance Criteria

- Test: a create that fails before creating anything, with its coordinator still alive, does not block `remove` of another slot; `status` lists it as a failed create with the cleanup command.
- Test: `recover --slot S --abort-create` drops such a journal when called by another coordinator of the project, and still refuses for a create that did create something while its actor is live.
- The refusal and remedy text for a blocking journal name the ancestry requirement and the exact command.

# Notes

Found in the Orc trial debugging pass (#2108, owner request 392, 2026-11-26). The trial team's wrapper gained `slot-recover` as the immediate workaround; the missing `/usr/sbin` on its PATH was the original cause of the failed create.
