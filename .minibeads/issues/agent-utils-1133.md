---
title: 'restore-drill-proof: a reboot-free drill proves the single restore script end to end'
status: open
priority: 1
issue_type: feature
created_at: 2026-11-09T13:45:55.621866979+00:00
updated_at: 2026-11-09T13:46:07.267374838+00:00
---

# Description

The deterministic host restore and its four scope fixes are only believable with a proof that needs no real reboot. Today the restore rehearsal is unit-level with scripted host facts; nothing exercises the single host command end to end against a scratch registry, a scratch terminal server, and a scratch slot project, and the only full drill on record is an unapproved proposal.

Without that proof, the next reboot is the test. The 2026-11-09 reboot showed the cost: certificate timing, dead candidates, slot rebinds, include-list drift, and rerun truthfulness each failed in a way a rehearsal would have caught, and each needed manual recovery while the host was down.

# Design

Build a reboot-free drill that runs the real single host command against fully scratch state and asserts on state and reports, never on wall-clock probe timings (the failure mode that forced the cross differentials exclusive).

Real: the host restore command and the per-registry restore it calls; a scratch registry with planted records and restore configuration; a scratch terminal server in a named test session on its own socket (never the live server); a scratch slot project with a real repository and slot lifecycle; the per-registry report files and exit codes; slot ownership read back through the slot manager.

Stubbed: the harness processes. Three or four stub lanes: a cheap fake harness that records its environment and stays alive, one profile in the style of the primary harness carrying an environment value that must survive revive, and one lane owning a slot. Boot identity is presented through the existing host-facts test override; the certificate path points inside the scratch tree.

Simulated reboot: stop only the scratch server and the stub harness processes by literal verified pid (each re-checked from its record immediately before the signal; never by pattern), present a different boot id, and start with the certificate missing, then have it appear after a few seconds while the host command waits.

Wire the drill into validation as a non-exclusive step (or a component test riding existing shards if that isolates cleanly): no exclusive label, no fixed port, scratch homes and sockets under a temporary directory, resource declaration of one unit. Classify its files so path selection runs it for restore changes without selecting everything.

Also write the runbook for a live drill without a reboot: the coordinator runs the same command against the real registry in a quiet window with the owner's approval, with pre-checks, an abort condition, and read-back of the table and reports. The runbook touches no live unit, bridge, or server beyond the restore itself.

# Acceptance Criteria

- Happy path: after the simulated reboot, the single command exits 0; every stub lane is back, the profile environment value is present in the revived lane, the slot is owned by the new harness pid, and each revived lane has its resume brief queued.
- An immediate rerun is a truthful no-op: same exit class, replayed rows, no second brief, no second launch.
- Certificate variant: when the certificate never appears, the command exits 75 within its bound, names the remedy, and launches nothing that needs the certificate.
- Refusal variant: with one unrestorable lane, the command exits 74 naming that lane while the others come back.
- The drill runs green in validation without an exclusive waiver and leaves no process, socket, or directory behind; the live-drill runbook is recorded beside it and names its approval gate and abort condition.

# Notes

Proof for the deterministic script and the four scope issues filed alongside it. Reuses the existing fake-server and scratch-slot fixtures where they isolate cleanly; asserts on records, reports, ownership, and queued messages. The live drill is a separate operator act with owner approval; this issue ships its runbook, not its execution.
