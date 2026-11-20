---
title: 'wrkslots-machine-id-changed-host-alias: bind pre-reprovision owners after the machine identity changes'
status: closed
priority: 0
issue_type: bug
labels:
- wrkslots
created_at: 2026-11-19T23:48:16-05:00
updated_at: 2026-11-20T03:09:46-05:00
closed_at: 2026-11-20T03:09:46-05:00
---

# Description

[watermelon-full-preview] On 2026-11-19 the host rebooted into a reprovisioned image and its machine identity (`/etc/machine-id`) changed. Every slot owner recorded before the reprovision is now judged `indeterminate: the recorded owner belongs to a different machine identity` forever: `_process_state_legacy` (and the Rust mirror) compare the recorded `host_id` against the live one before the boot check, so the boot check that would prove an old-boot owner dead never runs. `transfer-owner` refuses because it only replaces an owner that is provably dead, `adopt` only binds new slots, and `recover-unbound-owner` records evidence without rebinding. All active slots on the host are stuck, their heartbeats cannot run, and they hold unpushed work.

# Design

- Add a host-local, coordinator-recorded machine-id alias: a file outside the shared registry state, so no old reader ever parses it. No field, journal key, or event kind is added to the shared state; readers that predate this change keep failing closed (`indeterminate`), exactly as today. The alias path and format are reported to the coordinator before implementation, because this loosens an owner-death proof.
- A new verb, `wrkslots host-alias add --previous-host-id OLD --coordinator-authorized --coordinator-pid P`, verifies before writing: the coordinator is live and authorized; OLD differs from the current machine-id; the current boot differs from every boot recorded under OLD (scanning the shard's active records); and the short hostname matches the shard. The evidence (who, when, old and new ids, hostname, boot ids seen) is recorded in the alias file.
- The judge in both editions treats a `host_id` equal to a recorded alias of this host as this host, so the existing boot check then proves an old-boot owner dead. Everything else is unchanged: a `host_id` that is neither current nor an alias stays `indeterminate`, and an alias whose recorded boot equals the current boot never counts.
- The Rust edition (status/doctor liveness, classify) reads the same alias file, with a parity test against the Python edition.
- Document the recovery in the user guide under an "after a reprovision" section.

# Acceptance Criteria

- A slot recorded under machine-id A, with the host now at B on a new boot: before the alias, `transfer-owner` refuses; after `host-alias add --previous-host-id A`, it succeeds. A `host_id` C that was never aliased is still refused.
- `host-alias add` refuses an alias whose boot equals the current boot, refuses OLD equal to the current machine-id, and refuses when the hostname does not match the shard.
- `transfer-owner`, `adopt`, and `recover` work normally for aliased slots, and a cold review precedes landing.
- Tests drive the exact defect with condition-based waits only and pass on a host above 75% CPU.

# Notes

[watermelon-full-preview] mu-hostalias lane, 2026-11-19. Urgent: every slot owner on the host is unbindable until this lands. Alias file path and format are reported to the coordinator before implementation.

## Resolution

[watermelon-full-preview] Landed on github/main as 448403ae (tree c8f40f45, replay of e0499063 over c6082172). `wrkslots host-alias add --previous-host-id OLD --coordinator-authorized --coordinator-pid P` records previous machine identities in `$XDG_STATE_HOME/wrkslots/host-aliases.json` (host-local, 0600, outside every registry; no shared-state field, journal key, or event kind added). Both judges count an aliased identity as this host only across a boot change; a current-boot record under a previous identity stays indeterminate, so no live owner reads dead (cold-review probes 3d/3b2). Two cold reviews: first NOT SAFE (live owner read dead), fixed; second SAFE. Gate e0499063: every wrkslots step green; the 147 failed steps are waived environment failures (Chromium cannot start, libnspr4.so missing since the reprovision) and 4 no-results are learned-cap OOM kills. Overlap with the census redo was handled by verifying the exact replayed tree (56 named tests, Rust parity, clippy, strict mypy) and landing with --allow-overlap --verified-tree.
