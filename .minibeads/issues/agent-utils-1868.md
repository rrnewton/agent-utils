---
title: 'wrkslots-finish-without-handoff-dead-end: finish without a prior handoff closes every exit'
status: closed
priority: 2
issue_type: bug
labels:
- wrkslots
- lifecycle
created_at: 2026-11-18T12:30:00+00:00
updated_at: 2026-11-19T02:10:00+00:00
---

# Description

[watermelon-full-preview] Reported 2026-11-18 on build f1697dc3: `finish --discard-unaccounted --discard-ignored --validation ... --limitation ...` on a slot with NO prior `write-handoff` records a finish but writes no handoff sidecar. While the owner (here the coordinator itself) still lives, every exit is then closed: `remove` refuses (owner alive), `read-handoff` refuses (no sidecar), `write-handoff` refuses (finish recorded, handoff immutable), and `release` refuses (no sidecar). The slot can neither be removed, released, nor given the handoff its finish already implies.

# Design

Smallest safe combination, to be confirmed against current main: `finish` either writes the handoff sidecar itself or refuses when no prior handoff exists, AND `remove` accepts a recorded finish when the requesting coordinator IS the recorded owner (owner-consented). No new persisted field is introduced; any new persisted field would be reported to the coordinator before gating and gated by `writer_ceiling >= 3` (the #1788 mechanism) or not persisted, because pinned old readers must not see an unknown key.

# Acceptance Criteria

- The exact reported chain (`finish` without a prior handoff, then `remove`/`read-handoff`/`write-handoff`/`release` while the owner lives) reaches a usable exit instead of four refusals, in a deterministic end-to-end test through the real CLI (both editions where the surface exists).
- A `finish` that would strand a slot either writes its sidecar or refuses before recording, and `remove` by the recorded owner of a finished slot proceeds.
- Existing finish/handoff/remove/release suites pass.

# Notes

One of three related lifecycle dead-ends reported together; see #1869 wrkslots-adopt-reports-success-binds-nothing and #1870 wrkslots-transfer-owner-held-dead-owner. Whether current main already changes this behaviour is checked before the fix and recorded in the resolution.

## Resolution

[watermelon-full-preview] Reproduced on current main before fixing: finish recorded its handoff in the slot record and wrote no sidecar, after which remove, read-handoff, write-handoff and release all refused. Current main had not changed it. Landed as 817b4fb4: `write-handoff` (Python and Rust) no longer refuses on a recorded finish when no sidecar exists yet, so the slot publishes its handoff once, afterwards. The finish record is never modified; a different second sidecar still refuses as immutable, and an identical re-write stays idempotent. The smallest safe combination was chosen over having finish write the sidecar itself (the sidecar schema admits only `write-handoff` and legacy sources, which pinned old readers enforce) and over a second owner-consent path in remove (release plus owner-consented removal already provide it once the sidecar exists). No new persisted field. Covered by an end-to-end test of the reported chain (finish, late write-handoff, release, read-handoff, remove, owner alive) and a Rust differential test with a finish already recorded.
