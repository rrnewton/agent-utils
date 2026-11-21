# wrkslots transition model (`transitionmodel.py`)

Problem: wrkslots keeps a whole registry consistent across many
short-lived processes that read lock-free, work for minutes, then
commit under a lock — and any of them can die mid-write. Two
wedges from that shape reached production in one day (#1766,
#1807). This model exists so the next one is found by a search
instead: it is an executable, explicit-state model of the
registry's persistent state and locks, explored exhaustively over
concurrent actors and crashes.

Outcome: `wrkslots.transitionmodel` plus
`tests/test_transition_model.py` prove, mechanically and in
validation, that (a) the current-main transcription forks the
revision chain exactly as #1807 did, (b) the pre-#1766 phase
order deletes a subvolume under a locked registration, (c) the
fix variants close both, and (d) every other resting state the
search reaches either rests clean, reaches clean through
wrkslots commands, or is one of the named classes in the report
at `ai_docs/reports/research/2026-11-18-wrkslots-model-check.md`.

Dependencies: Python standard library only. Nothing in the
runtime imports this module; like `lifecyclemodel.py` (#1568) it
is a checked abstraction. That module models one slot's
lifecycle and folds cross-slot effects into scenario flags; this
one models the dimension it folds away — two slots, concurrent
actors with program counters, the revision chain, journals, the
mutation lock, and crashes. A Python breadth-first explorer is
the checker because no TLA+/TLC toolchain is available offline
and validation must not download one (the same call #1568 made);
the phase tables below are the temporal reading of the model.

## Run it

```bash
# The gated assertions (what validation runs): the fast subset —
# both acceptance oracles, fix closure, the fork-wedge shapes,
# happy paths, and a one-step smoke over every scenario (~13 s)
python3 -m pytest py/wrkslots/tests/test_transition_model.py -q

# Full battery, opt-in, run by hand: every scenario explored
# exhaustively with the candidate assertions (~95 s)
WRKSLOTS_MODEL_FULL=1 python3 -m pytest \
    py/wrkslots/tests/test_transition_model.py -q

# Same explorations without assertions, traces printed
python3 py/wrkslots/transitionmodel.py
```

The split is deliberate (fast, high-signal gating tests): if a
transcription stops matching the code, the fast oracles redden
first; the full battery exists to re-derive the report's
counts and to search wider after a model change.

One scenario from a REPL or a scratch script:

```python
import sys; sys.path.insert(0, "py")
from wrkslots import transitionmodel as tm

ex = tm.explore(tm.Scenario())          # current main
forks = ex.violations("forked-chain")   # sticky safety ghosts
best = ex.shortest(forks)               # minimal counterexample
print(ex.trace(best))                   # action names, in order
print(len(ex.wedges()), len(ex.manual_terminals()))
```

`Scenario(fixes=...)` selects code variants (see below);
`reg_a` sets slot A's git registration lock (`none`, `own`,
`foreign`); `b_present`, `b_unbound`, `misjudged_a`,
`retained_writer`, `contention`, `max_heartbeats`, and
`disable` shape the start state and actor set. Exploration is
breadth-first with shortest-trace parents, bounded by
`max_states` (default 900,000; exceeding it raises instead of
truncating silently).

## What is modelled

Persistent state (`State`): event-log tip revision and whether
the chain is still continuous; replay-derived per-slot heartbeat
versions and active rows; the `ACTIVE.<machine>.json` snapshot
as a separate, possibly lagging copy; per-slot heartbeat expiry,
owner truth vs proof, hold, handoff; per-slot journals (agent
finish event-pending/file, validate finish prepared/fenced,
create before/after its durable row) with actor liveness; the
validation-batch seal (live/dead, target); storage
(canonical/fenced/deleted/absent); git registration locks
(none/own/foreign); archive rows; a leftover partial-write tmp;
a config-TTL-edited flag; the registry-identity token; the
mutation-lock holder; and a sticky violation ghost.

Actors are transcriptions, each step citing `py/wrkslots/cli.py`
at the modelled revision. Line numbers drift with the file; the
function names are the stable reference.

| Actor | Code | Phase shape kept |
|---|---|---|
| PreparedRemove(A) | `_prepare_retirement_removal`, `_assert_prepared_removal_current`, `_begin_finish` | lock-free load → salvage → identity capture → lock → identity + snapshot-record compare → append with passed previous → snapshot from advanced state → finish journal (event, then file+fence) → lock-free delete → fresh-load completion |
| Heartbeat | `_cmd_heartbeat` | lock → append → snapshot → unlock; no lock-free state trusted |
| Create(B) | `_cmd_create` | lock → journal → worktrees → publish row → clear journal |
| ValidateBatchRemove(A) | `_validate_subvolume_delete_first`, seal handling | seal → finish journal → fence → classify registrations (fix variant) → delete → prune → archive; pre-fix variant deletes first |
| PrevalidatedAdopt(B) | `_run_prevalidated_mutation`, `_cmd_adopt` | identity capture → lock-free prepare → lock → identity compare → commit or retry (5 rounds, then busy); contention runs only |
| Recover (edges) | `_cmd_recover`, `_recover_finish`, `_recover_create` | atomic per journal kind; re-gates finish on heartbeat/hold/owner; create refuses on config-TTL drift; seal retire when orphaned |
| RecoverTailEvent (edge) | `_cmd_recover_tail_event` | truncates only a discontinuous tail with no journal, seal, or partial write outstanding |
| Environment | — | crash of any actor at any phase boundary (locks free on death), crash mid-atomic-write leaving a tmp, clock tick, owner death/probe, a misjudged owner's renewal, hold/unhold, config TTL edit/revert, hand git unlock (outside wrkslots) |

The append step is generic shared machinery: an event lands with
whatever previous revision its caller passes, and load-time
continuity is judged afterwards — no transition mentions a
historical bug. Code variants (`Scenario.fixes`) change only
which fields a transcribed check compares or the order of
transcribed phases:

| Variant | Meaning | In |
|---|---|---|
| `pre-delete-classify` | registrations classified before the subvolume delete (#1766 fix) | current main |
| `rebase-on-advance` | prepared remove commits from the current log state (#1807 fix) | unlanded fix |
| `stale-previous-refusal` | the append writer refuses a passed previous that is not the log end (#1807 defence) | unlanded fix |
| `assert-compares-revision` | the under-lock assert also compares revisions (proposed minimal guard, for attribution) | neither |

Checks over the reachable graph: sticky safety ghosts
(`forked-chain`, `deleted-while-locked`); wedges (not clean, no
path to clean at all); manual terminals (clean reachable only
through hand actions outside wrkslots); dead-actor journals no
legal path clears; adopted-vs-busy reachability under
contention. `is_clean` and `is_resting` define the resting
states an operator meets between commands.

## Bounds

Two slots (A, B) on one machine. Each actor runs once, except
Heartbeat in contention runs (capped by `max_heartbeats`).
Heartbeat versions cap at 2; hold toggles at 2; the renewal of
a misjudged owner fires once (the expiry-crossing case recover
observes). Revisions grow only by modelled append steps.
States are quotiented where only dead bookkeeping differs (the
identity token restarts when no actor holds a capture; finished
actors' locals are dropped) — equality of captures is the only
observable. Battery sizes at the modelled revision: roughly
24k–62k states for the single-registry scenarios, 137k–149k
for create-recovery and misjudged-owner, 94k for isolated
contention.

## Not modelled

Recover is atomic (a crash inside recover re-enters the same
journal state; its internal lock scopes and the roll-forward
repair are folded). One machine only: no cross-machine shards,
no other-registry effects. Owner proof is a three-valued fact;
the census/proc machinery that produces it (and its accuracy)
belongs to `lifecyclemodel`'s truth/proof axis. The archive
revision chain folds into the active append that accompanies
it. Eden slots, image/bind machinery beyond the delete point,
salvage contents, and wall-clock durations are absent: time is
one expiry tick. Lock waiting is unmodelled beyond enabledness
(a held lock disables acquirers; timeouts and the intent
turnstile do not create state). A misjudged owner's heartbeat
is an atomic edge, so its own append/snapshot crash window on
the target slot is reasoned about in the report, not searched.
The Rust edition is not transcribed; where its shape differs
materially (the remove writer reloads fresh state and compares
chain length) the report says so from code reading.

## TLA+ mirror checked with TLC (#2043)

`spec/WrkslotsTransition.tla` is the same model in temporal
form — same two slots, actors, program counters and phase
order, action names identical to the Python model — plus two
dimensions the Python model folds away: finish-journal
*content* (inline vs sidecar vs reloaded-expanded form, the
#1757 64-entry inline limit as the `RefsLarge` constant) and
the deferred out-of-lock deletion window whose completion
compares the reloaded journal against the in-memory one
(the #894/#1735 shape behind #2041). `spec/run_tlc.sh` runs
TLC over the `spec/WrkslotsTransition-*.cfg` configurations;
it is opt-in only — validation never downloads a toolchain
and never runs TLC, and the Python explorer above stays the
gated cross-check. Toolchain used for the first check:
tla2tools.jar v1.7.4 (official tlaplus release asset,
2,274,532 bytes, sha256
`936a262061c914694dfd669a543be24573c45d5aa0ff20a8b96b23d01e050e88`;
that release publishes no checksum file), run with the
host's Java 21.

First TLC results (2026-11-21; an expected FAIL is the
counterexample doing its job):

| Config | Result | Distinct states (depth) |
|---|---|---|
| `current` (current main, small refs) | all invariants pass | 133,044 (30) |
| `fork` (`NoViol`, current main) | FAIL: `forked-chain`, `ok=0` — RmLoadFree → RmSalvage → HbAppend → RmCaptureIdentity → RmAssert → RmAppendBasis (#1807) | 5,369 (10) |
| `pre1766` (no fixes, foreign lock) | FAIL: `deleted-while-locked`, journal `vfence` left (#1766) | 765 (6) |
| `allfix` | pass, incl. `NoViol` | 126,660 (30) |
| `shapes` / `shapes2` (C1 shapes) | FAIL: buried fork + finish journal; fork at tip + dead seal (#1830) | 15,329 / 15,729 (12) |
| `sidecar` (`RefsLarge`, raw compare) | FAIL: `ReloadEqualsMemory` at the fence step (#2041 class) | 73 (4) |
| `sidecar-form` | FAIL: `NoFormStuck` — deletion done, completion refused, journal `ffence` left (RECOVERY REQUIRED) | 22,115 (13) |
| `sidecar-fixed` (`NormalizeCompare`) | pass | 133,044 |
| `contention` | FAIL: `NoAdoptBusy` witness reachable; `contention-safety` (`NoViol` alone) passes | 402,732 (39) |
| `liveness` (current main) | FAIL: `OneRecoverCompletes` — C1 interrupted states no fair recover completes | 133,044 |
| `liveness-fixed` (all fixes) | pass under recover + hand-exit + actor-exit fairness | 126,660 |
| `globaljournal` (disjoint create, absent B) | pass: no create commit inside the live finish window | 247,990 (32) |
| `globaljournal-witness` | FAIL: `NoCreateBusyInWindow` — the busy refusal is reachable in the window | 6,704 (12) |
| `globaljournal-control` (hypothesis toggle) | FAIL: `NoWindowJournal` — with the gate removed the window commit appears | 6,705 (12) |
| `globaljournal-adopt` | pass: no adopt commit inside the live finish window | 1,245,746 (45) |
| `globaljournal-adopt-witness` | FAIL: `NoAdoptBusyWithFinish` — busy refusal reachable beside a live finish journal | 33,840 (13) |

The two explorers agree on every known result: the #1807
fork (same interleaving order), #1766, the C1/#1830 wedge
shapes, the contention witness, and — new in the TLA+ model —
the #2041 sidecar class, which appears exactly when
`RefsLarge` holds and the completion compares raw forms,
and closes when the comparison expands both sides first.

The global-journal hypothesis first suspected for #2041 —
a create or adopt on a disjoint slot committing during the
out-of-lock finish window — is refuted in the model as it
was at the test seam: a commit first meets
`_assert_no_journal`, a live finish journal raises
`_JournalBusy`, and the command refuses busy with nothing
written. TLC confirms the refusal edge is reachable in the
window (the two witness configs) and that no window commit
exists over the full spaces; removing the gate
(`WindowWriteHypothesis`, the control config) makes the
window commit appear, so the check has teeth. Adopt is
additionally blocked by identity staleness: a capture taken
before the window no longer matches inside it.

The older `spec/WrkslotsLifecycle.tla` (#1568) was also
checked with this toolchain. No record of a prior TLC run
exists — the docs present it as a mirror whose gated checker
is the Python explorer — and as first checked it did not
complete one: TLC's deadlock check stopped at depth 2
(5 states) on the stray-storage start state, which has no
successor and is not terminal. The spec now stutters there,
exactly the leaf the Python explorer already treats it as;
a default TLC run explores 11,795 distinct states, `Safety`
holds in the baseline config, and `Liveness` fails as its
config comment expects (no finisher in the baseline).

## Seeded fuzz against the real code (#2043 Phase 2)

`tests/lifecycle_fuzz.py` generates seeded random command
sequences over two slots in a scratch registry, injects
crashes at the CLI test seams between journal and event
writes, and decodes the real state after every step (event
chain, ACTIVE snapshot, journals including containing-refs
sidecars, seal), judging it against the invariants above.
`tests/test_lifecycle_fuzz.py` gates a fast subset
(seeds 1..4, bounded steps, condition-based waits); the long
soak is opt-in by hand. Live-owner vocabulary (adopt,
heartbeat, transfer-owner through the mock-agent tree) is
covered by `tests/e2e_stress.py` and joins this harness only
where the scratch project can drive it directly.
