# wrkslots formal models (TLA+)

The registry's persistent state, locks and crashes are modelled
in TLA+ under `py/wrkslots/spec/` and checked with TLC. The
Python explicit-state explorers that used to live here
(`transitionmodel.py`, `lifecyclemodel.py`, #1568/#1828) were
deleted in #2090 (owner direction: one model, in TLA+); their
findings (#1807, #1766, C1/#1830, C2/#1831, contention) are
preserved as TLC configurations below and in
`ai_docs/reports/research/2026-11-18-wrkslots-model-check.md`.

## Specs

| Spec | Models |
|---|---|
| `spec/WrkslotsTransition.tla` | Two slots, concurrent actors with program counters, the revision chain, ACTIVE snapshot, per-slot journals (inline/sidecar/expanded forms), the mutation lock, seals, crashes (#1828/#2043) |
| `spec/WrkslotsLifecycle.tla` | One slot's lifecycle: row, storage, owner proof, handoff, hold, journal, validate publication (#1568) |
| `spec/WrkslotsLock.tla` | Lock acquire/hold/release for a repeating recover-absent hog vs heartbeat/create under a non-FIFO flock (#2090) |

## Check it

Gated: `py/wrkslots/tests/test_tlc_gate.py` runs the small
config set in validation when a checksum-pinned jar is
provisioned (`TLA2TOOLS_JAR`, else
`~/.local/share/tla2tools/1.7.4/tla2tools.jar`, sha256
verified); without a jar it skips by name and the gate has no
model-check coverage on that host. Manual, opt-in (never run
by validation, never downloads):

```bash
TLA2TOOLS_JAR=~/.local/share/tla2tools/1.7.4/tla2tools.jar \
  py/wrkslots/spec/run_tlc.sh
```

## Recorded verdicts (TLC 2.19)

Transition (states = distinct): `current` passes (133,044);
`fork` fails with the #1807 interleaving (RmLoadFree →
RmSalvage → HbAppend → RmCaptureIdentity → RmAssert →
RmAppendBasis); `pre1766` fails deleted-while-locked (765);
`allfix` passes (126,660); `shapes`/`shapes2` fail with the
C1 wedge shapes; `sidecar` fails ReloadEqualsMemory at the
fence step and `sidecar-form` fails with the completion
refused and journal left (the #2041 class), both closing
under `NormalizeCompare` (`sidecar-fixed` passes);
`contention` reaches the busy witness while
`contention-safety` passes; `liveness` fails on current main
(C1) and `liveness-fixed` passes; the global-journal window
configs confirm a disjoint create/adopt refuses busy beside
a live finish journal and never commits in the window.
Lifecycle baseline: Safety holds, Liveness fails (no
finisher), 11,795 states. Lock: pre-#2088 starves (temporal
counterexample, 48 states), post-#2088 passes (30 states).

Bounds mirror the deleted explorers': two slots, heartbeat
versions ≤ 2, adopt 5 rounds, one expiry tick; action names
are the old Python model's, so counterexamples read the same.

## Seeded fuzz

`tests/lifecycle_fuzz.py` checks the same invariants against
the real code (see FUZZ_INVARIANTS.md for the shared
predicates and decode contract).
