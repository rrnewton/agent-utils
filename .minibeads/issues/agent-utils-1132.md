---
title: 'restore-already-restored-truth: a rerun in the same boot replays recorded outcomes instead of reading as a fresh success'
status: open
priority: 1
issue_type: bug
created_at: 2026-11-09T13:45:50.286593800+00:00
updated_at: 2026-11-09T13:45:55.614924813+00:00
---

# Description

Found during the 2026-11-09 host reboot. The boot-time restore left lead agents down, but re-running restore in the same boot without force read as a fresh no-op success: the host summary printed empty completion fields and the host command exited zero. The recorded per-boot report showed refusals, but nothing in the rerun path surfaced them.

The per-registry restore short-circuits a same-boot rerun when the recorded report says complete, returning a minimal already-restored result. Completion today does not require zero refusals, so a boot restore that finished with refused lanes still short-circuits, and the caller maps the short-circuit to exit zero. A caller that keys its exit code and table off the rerun therefore reports success for a boot whose restore failed in named ways.

Impact: operators and boot units cannot trust a green rerun. The failure is only visible by opening the per-boot report file by hand, which is how it was missed on the morning.

# Design

A non-force rerun in the same boot replays the recorded outcomes instead of returning an empty success. The already-restored result carries the recorded coordinator and agent rows (or the full recorded report) labelled as a replay, including refused lanes and their reasons, plus the recorded completion fields.

Exit class follows the recorded run: zero only when the recorded restore completed with no refusal; 74 when it completed with named refused lanes; 75 when it did not complete for a temporary reason. The host command renders replayed rows as replayed, never as fresh revives, and a dry run reports would-complete from the same recorded facts. Force remains the explicit way to attempt the work again.

The recorded report is read from the per-registry restore-reports location, so one registry's replay can never present another registry's boot outcome.

# Acceptance Criteria

- After a boot restore that completes with two refused lanes, a non-force rerun prints those two lanes with their recorded reasons, labelled as replayed, and exits 74.
- After a clean boot restore, a non-force rerun replays the revived rows and exits 0; after an incomplete temporary failure it exits 75.
- A force rerun still attempts the work again and writes a fresh report; the replay path never consumes a revive transaction.
- Two registries restored in the same boot replay their own reports independently; neither presents the other's rows.
- Coverage drives the real restore entry point twice in one simulated boot (fail then rerun, clean then rerun) rather than asserting on the report file alone.

# Notes

Scope gap (d) from the 2026-11-09 reboot restore; the restore agent's note names the same slug. Touches the same-boot short-circuit and exit mapping in both agentctl editions plus the host table's replay rendering. Depends on the per-registry report location so the replay reads the right file.
