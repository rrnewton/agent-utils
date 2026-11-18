# wrkslots transition model check (#1828)

Date: 2026-11-18. Lane: mu-modelcheck. Code modelled:
`py/wrkslots/cli.py` at github/main `e16fbeb9` (the #1766 fix
landed, the #1807 fix still unlanded on its lane branch at
modelling time, so current main *is* the #1807 pre-fix code
path). Artefacts: `py/wrkslots/transitionmodel.py` (model and
checker), `py/wrkslots/tests/test_transition_model.py` (gated
assertions), `py/wrkslots/TRANSITIONMODEL.md` (how to run it,
coverage, bounds). No product code was changed. The four
candidate findings were filed from this lane at the
coordinator's instruction as #1830–#1833 (§8).

## Verdicts

| Question | Verdict | Where |
|---|---|---|
| #1807 forked revision chain | **Rediscovered** unguided on current main; each fix variant closes it | §2 |
| #1766 deleted-while-locked | **Rediscovered** on the pre-fix phase order; landed fix closes it | §3 |
| C1 broken log + outstanding journal/seal | **Confirmed wedge** (new): no sanctioned verb can run | §4.1 |
| C2 config `heartbeat_ttl_seconds` edit | **Confirmed refusal loop** (new): recover, abort, and quarantine each refuse or point at another verb; only reverting the config exits | §4.2 |
| C3 finish owner heartbeats again | **Refuted** as a wedge: reachable, recover refuses while fresh, expiry re-opens recover | §5 |
| C4 hold vs finish-recovery | **Refuted**: unreachable co-occurrence; the recover gate is defence in depth | §5 |
| C5 snapshot divergence after crash | **Refuted** as an independent wedge; confirmed as a #1807 amplifier | §5 |
| Contention (#1276 shape) | Bounded witness: the 5-round budget is exhaustible under churn; commit also reachable; converts to a clean busy refusal, not corruption | §6 |

Every wedge in every scenario sits on a broken revision chain
(`ok=0`); no scenario produced a wedge on a loadable log. That
is the single most useful negative result here: inside the
modelled bounds, journal/seal/hold/TTL machinery alone never
deadlocks the registry — it takes chain damage (§4.1) to remove
the last exit.

## 1. Method

The checker is a Python explicit-state breadth-first explorer
(stdlib only), following the checked-abstraction pattern of
#1568 (`lifecyclemodel.py`): a hashable state tuple, scenario
constants for code variants, a sticky violation ghost, and
shortest-trace counterexamples. TLA+/TLC was rejected for the
same reason #1568 rejected it: no toolchain is available
offline (verified on this host: no `tlc`, no `tla2tools.jar`)
and validation must not download one; an unchecked TLA text
would be documentation, not evidence.

Actors are transcriptions of each command's phase structure,
not of its outcomes. The load-bearing discipline, per the
coordinator's acceptance guard: no transition encodes a known
bug. An event append takes whatever previous revision its
caller passes and lands; continuity is judged at load, exactly
as `_fold_active_record_event` judges it (cli.py:10067). The
under-lock assert compares exactly the fields
`_assert_prepared_removal_current` compares (cli.py:61471):
registry identity, journal fingerprint, this slot's record as
read from the ACTIVE snapshot, lock-scope fingerprint. Code
variants change *which fields a transcribed check compares* or
*the order of transcribed phases* — the landed #1766 fix, the
unlanded #1807 fix, a proposed one-field guard — and the search
decides what follows. The known production traces are used
only as test oracles.

State and bounds (full detail in TRANSITIONMODEL.md): 2 slots
on one machine; actors PreparedRemove, Heartbeat, Create,
ValidateBatchRemove, PrevalidatedAdopt (contention runs),
atomic Recover/RecoverTailEvent edges, and an environment
offering a crash of any actor at any phase boundary (plus a
mid-atomic-write crash leaving a partial tmp), clock ticks,
owner death, a misjudged owner's renewal, hold/unhold, config
edits, and hand git unlocks. Each actor runs once except
contention heartbeats (capped). Scenario battery and sizes:

| Scenario | States | Forks | Deleted-while-locked | Wedges |
|---|---:|---:|---:|---:|
| current-main reg=none | 24,156 | 1,014 | 0 | 624 |
| current-main reg=own | 40,980 | 1,872 | 0 | 1,152 |
| current-main reg=foreign | 40,980 | 1,872 | 0 | 1,152 |
| pre-1766 reg=own | 57,396 | 1,932 | 15,372 | 1,608 |
| pre-1766 reg=foreign | 53,508 | 1,908 | 11,508 | 1,464 |
| all-fix reg=own | 38,772 | 0 | 0 | 0 |
| all-fix reg=foreign | 38,772 | 0 | 0 | 0 |
| stale-refusal only | 23,142 | 0 | 0 | 0 |
| retained-writer | 23,142 | 0 | 0 | 0 |
| revcheck only | 22,962 | 0 | 0 | 0 |
| create-recovery | 136,608 | 11,028 | 0 | 7,800 |
| misjudged-owner | 142,626 | 4,404 | 0 | 2,496 |
| contention (isolated) | 94,362 | 0 | 0 | 0 (busy: 630) |

Reproduce with `python3 py/wrkslots/transitionmodel.py`
(prints minimal traces per class) and the checked tests:
the gating fast subset by default (~13 s: both acceptance
oracles, fix closure, wedge shapes, smoke), the full battery
under `WRKSLOTS_MODEL_FULL=1` (~95 s). Wedge counts include
mid-flight states whose every future is wedged; the report
quotes *resting* end states (no lock held, no actor
mid-program) as the operator-visible forms.

## 2. #1807 rediscovered (acceptance)

Shortest crash-free counterexample on current main:

```
RmLoadFree -> RmSalvage -> HbLock -> HbAppend -> HbWriteActive
  -> RmCaptureIdentity -> RmLock -> RmAssertCurrent
  -> RmAppendBasis
```

Reading against the code: `_prepare_retirement_removal` loads
the whole registry lock-free at revision R (cli.py:61573);
salvage runs lock-free (61740); the other slot's heartbeat
appends R→R+1 under its own locks (44295, 44363); the prepared
removal captures registry identity only now, in its return
statement (61767), while its saved state is still revision R;
under the lock, the assert (61471) passes because everything
it compares postdates the heartbeat or concerns only the
target slot; the commit calls `_write_active_state` with the
stale state as `previous=`, which a passed previous is trusted
by construction (10600–10606, replayability judged against
that same previous at 10641), so a second R→R+1 event lands
and ACTIVE is rewritten from the stale registry — the model
independently exhibits the snapshot rollback too (states with
`snap_hb[B] < log_hb[B]` after `RmWriteActive`, crash-free).
Every later load refuses at 10067.

Two discriminating controls, both asserted in tests:

- No fork trace has the other slot's append *after* the
  identity capture — that direction defers, as in production.
  The search also forks the chain off *other* appends in the
  salvage window (a create-recovery completion, an adopt
  commit), confirming the defect is the window, not heartbeat.
- The retire-pending retained-writer shape (61662) does not
  fork: its stale sequence collides with the existing event
  file (9796) and refuses. The fresh-writer shape (62497) is
  the forking one. The Rust remove writer is immune for a
  stronger reason (code-read, not modelled): it reloads fresh
  state under the locks and requires an identical event-chain
  length before deriving `previous_revision`
  (`rs/wrkslotsd/src/remove_write.rs:4277`, `:4370`).

Fix closure, each variant independently sufficient in the
model: rebase-on-advance, stale-previous-refusal,
assert-compares-revision, retained-writer — all yield zero
violations and zero wedges. The unlanded fix combines the
first two; the model agrees either alone prevents the fork,
which is the right defence-in-depth reading (rebase keeps
removals useful under churn, the refusal keeps a future caller
from reintroducing the shape).

A bare fork at the tip with nothing outstanding heals through
the modelled `RecTailTruncate` (recover-tail-event): 48
resting-clean states carry the fork ghost as history in the
default scenario. The wedges of §4.1 are what remains when
that exit is also barred.

## 3. #1766 rediscovered (acceptance)

With `pre-delete-classify` removed (the parent of the fix),
the search deletes the subvolume under a locked registration
by two routes. Shortest, own lock via recovery:

```
VbSeal -> VbFinishPrepared -> CrashVb -> RecFinishDeleteLocked
```

and the direct batch route, foreign lock:

```
VbSeal -> VbFinishPrepared -> VbFence -> VbSkipClassify
  -> VbDeleteSubvolume -> VbRefuseStillRegistered
```

ending resting at `storage=deleted, jrnl=vfence, seal=dead,
reg=foreign` — the production residue (finish journal plus
paired seal over a deleted checkout still registered locked),
from which every wrkslots verb refuses and only hand git
commands exit. In model terms these end states are *manual
terminals* (the hand unlock exists outside wrkslots), which
matches the production remedy exactly; the violation ghost
(`deleted-while-locked`, 15,372 states at reg=own) is what
separates pre-fix from fixed. With the landed fix transcribed,
the ghost is unreachable at any registration value: own locks
are dropped before the delete (cli.py:58989–59025), foreign
locks refuse with storage intact and remain manual terminals
*by design* (wrkslots never drops a foreign lock, 22852; the
documented remedy is one hand unlock, then recover prunes and
archives, 59011). The fixed model's foreign manual terminals
all have storage present or fenced — never deleted — which is
the acceptance line the tests assert.

## 4. Confirmed new findings

### 4.1 C1: a broken log plus any journal or seal has no sanctioned exit

Every wedge in the battery is a fork descendant in one of
four resting shapes (default scenario counts over all states,
not only resting):

| Shape (ok=0) | Fork at tip | Journal | Seal | States |
|---|---|---|---|---:|
| fork + journal + seal | yes | yes | yes | 216 |
| fork + seal only | yes | no | yes | 108 |
| buried fork + journal + seal | no | yes | yes | 132 |
| buried fork + journal | no | yes | no | 168 |

Minimal traces. Buried fork with the removal's own
event-pending finish journal (`fpend` — the journal event
appended after the fork is what buries it):

```
RmLoadFree -> RmSalvage -> HbLock -> HbAppend
  -> RmCaptureIdentity -> CrashHb -> RmLock
  -> RmAssertCurrent -> RmAppendBasis -> RmWriteActive
  -> RmJournalEvent -> CrashRm        # resting: ok=0 tip=5
                                      # jrnl=fpend snap rolled back
```

Fork at the tip, no journal, one dead validation seal from an
unrelated crashed batch:

```
RmLoadFree -> RmSalvage -> HbLock -> HbAppend -> CrashHb
  -> VbSeal -> RmCaptureIdentity -> CrashVb -> RmLock
  -> RmAssertCurrent -> RmAppendBasis -> CrashRm
                                      # resting: ok=0 seal=dead
```

Why nothing runs, verb by verb (all verified by reading the
code at the modelled revision):

- Every ordinary command replays the log and refuses at
  cli.py:10067.
- `recover` validates global state — the same replay — before
  any seal or journal handling can retire anything
  (cli.py:90384, :90687; dispatch loads at :90689).
- `recover-tail-event --yes` first calls
  `_refuse_partial_state` with no seal allowance, then refuses
  on any outstanding journal ("the tail event may belong to
  it", cli.py:15131 region) and on any partial tmp. A dead
  seal therefore bars it even with no journal.
- `quarantine-journal` covers scoped *create* journals only
  (cli.py:14816), also calls `_refuse_partial_state`, and
  appends its receipt through `_event_writer`, which fully
  verifies the chain (cli.py:9812) — so even a create journal
  (the wedge trace in the create-recovery scenario ends with
  `jrnl=create1` on a forked log) cannot be set aside while
  the log is broken.
- Once any event lands after the fork — the forking removal
  itself appends evidence and journal events (cli.py:62528,
  :60241) — the discontinuity is no longer the tail, and tail
  recovery additionally refuses as broken-before-tail
  (cli.py:14989). Hand-moving the journal file aside is then
  not sufficient either; the log needs hand surgery.

Severity, honestly stated: today this needs a #1807-class
fork first, and the #1807 fix (once landed) removes the only
known fork source. The finding matters as defence in depth:
*any* future unreplayable tail — a new writer bug, a torn
write that passes the hash check shape, an older binary
writing beside a newer one — combined with the completely
ordinary coincidence of a crashed batch's dead seal or any
stranded journal, has no sanctioned repair. The repair verbs
were each designed against one fault; the model found the
composition unhandled. Fix sketches (candidate issue 1):
teach `recover-tail-event` a mode that sets aside the minimal
suffix (not just one event) with the same dry-run/echo/receipt
discipline, eligible when every outstanding journal's recorded
operation provably postdates the fork point or belongs to it;
additionally or alternatively, let a proven-dead *seal-only*
state be retired by a path that does not replay the log (the
seal file plus its paired-finish check is self-describing),
since the seal-only shape needs no journal judgment at all.

### 4.2 C2: a config TTL edit loops create recovery's refusals

Minimal trace (create-recovery scenario):

```
CrJournal -> CrashCr -> ConfigTtlEdit
  # resting: jrnl=create0 (dead actor), cfg_ttl drifted, ok=1
```

From that state: `recover` (including `--abort-create`)
raises `StateError: create journal heartbeat TTL differs from
configuration` (cli.py:68801) — the equality check precedes
both the durable-row clear and the abort branch (cli.py:68859).
`quarantine-journal` does not rescue it: for an incomplete
journal, classification succeeds without the TTL check, so
quarantine refuses as "which recover resolves" and points
back at recover (cli.py:14655 region); for a completed
journal the TTL `StateError` (cli.py:12953) propagates through
classification uncaught (quarantine catches `Refusal`, not
`StateError`). Create of the same slot refuses on the
outstanding journal. The only exit is reverting
`heartbeat_ttl_seconds` in config to the journal's recorded
value — a remedy no message names. In the model this is a
manual terminal (2,542 resting states in the create-recovery
scenario, all on a healthy chain), which understates it
slightly: an operator who edited the TTL *deliberately* (say,
fleet-wide) must temporarily un-edit it per stranded create.

Fix sketch with in-code precedent (candidate issue 2): hooks
already follow "the journal's recorded hooks govern, even when
the current configuration has since changed them; drift is an
audit note, never a refusal" (cli.py:12941). Apply the same
rule to the recorded TTL for recovery and abort — the journal
value is what the slot was created under — or at minimum
exempt the abort and quarantine paths from the equality check
and name the config-revert remedy in the refusal. The eden
create twin has the identical check (cli.py:41541).

## 5. Refuted candidates

**C3 — finish owner heartbeats again.** The re-gated state is
real and reachable (10,422 states in misjudged-owner with a
dead finish journal and a fresh heartbeat): `recover` refuses
to resume deletion (cli.py:69350), and the model agrees.
It is not a wedge: zero of those states are dead-journal
states — expiry always re-opens recover. A truly live owner
renewing forever is the gate working as designed (the owner
vetoes its own removal), bounded here to one renewal as
stated in the model bounds.

**C4 — hold vs finish-recovery.** `hold` requires no journal
on the target (cli.py:44546) and `_begin_finish` asserts
not-held under the lock (cli.py:60228); the first model draft
omitted the second check and the search immediately produced
hold+finish states through the salvage window, which is how
the omission was caught. With both transcribed, the
co-occurrence is unreachable in every scenario explored
(default, misjudged-owner, create-recovery: 0 states). The
recover-side hold refusal (cli.py:69229) is defence in depth
for state no command sequence creates. No issue.

**C5 — snapshot divergence after a crash.** Reachable as
expected (8,652 healthy-chain divergent states in the default
scenario: event appended, ACTIVE not yet rewritten, writer
dead). All of them reach clean — the next successful writer
or `_ensure_state_shard` (cli.py:9322) resynchronises from
the log — and no wedge in any scenario sits on a loadable
chain, so divergence alone wedges nothing. Its real role is
as a #1807 amplifier, reasoned here rather than searched
(the misjudged owner's heartbeat is atomic in the model):
the prepared-remove assert reads the target record from the
snapshot (cli.py:61493), so a target-slot change whose
snapshot write crashed while its event landed inside the
salvage window is invisible to that one check; pre-fix the
outcome is still the fork (the append's previous is stale),
and the rebase variant closes it by comparing against the
log-derived current state. No separate issue; noted on
candidate issue 1's context.

## 6. Contention: bounded starvation witness

In the isolated contention scenario (bound heartbeating
neighbour, unbound adopt target, other actors disabled), the
prevalidated adopter's 5-round budget
(`REGISTRY_VALIDATION_ATTEMPTS`, cli.py:6930, :7082) is
exhaustible: 630 busy states, e.g.

```
AdPrepareFree -> Tick -> RenewHeartbeatA -> AdLock
  -> AdRetryRound -> HbLock -> AdPrepareFree -> HbAppend
  -> CrashHb -> AdLock -> AdRetryRound -> AdPrepareFree
  -> EnsureShardSync -> AdLock -> AdRetryRound
  -> AdPrepareFree -> HoldA -> AdLock -> AdRetryRound
  -> AdPrepareFree -> UnholdA -> AdLock -> AdBusyExhausted
```

Each round is invalidated by some intervening write
(heartbeats, a snapshot resync, a hold toggle — anything that
bumps a registry file). The commit is also reachable
(asserted in the tests), so this is
per-invocation starvation, not impossibility, and the failure
mode is the designed one: a busy refusal with nothing written
(cli.py:7182), safe to retry as a whole command. Under
sustained registry-wide churn (many heartbeating slots), a
caller that retries immediately can in principle lose
repeatedly; the model bounds (one neighbour, four heartbeats)
show the shape, not a rate. Candidate issue 3 drafts this as
a low-priority liveness note (jittered whole-command retry
guidance, or a bounded wait for a quiet identity inside the
round loop), explicitly not a correctness defect.

## 7. Code-read notes (not checker findings)

Found while transcribing; stated here so they are not lost,
labelled as reading rather than search results.

- `transfer-owner --reopen` captures the coordinator identity
  before the lock and never re-asserts it under the lock
  (cli.py:43713–43878); the captured value is written into
  event evidence. Every sibling capture in that function is
  re-proven. Impact is evidence staleness, not state damage.
  Candidate issue 4.
- The agent prepared-remove path never enumerates
  second-repository registrations of its checkout (the #1766
  enumeration is validate-scoped). In the model, an agent
  slot archives past a foreign second-repository registration
  and leaves it dangling; whether the real path refuses,
  prunes, or leaves the same garbage was not verified by
  execution and is worth one targeted check before anyone
  files it. Not drafted as a candidate.

## 8. Candidate issues (filed as #1830–#1833)

Filed from this lane at the coordinator's instruction, in the
four-section shape, after checking no existing issue covers
each (the nearest neighbours are #1373, recover-tail-event's
origin, for candidate 1 and #1276 adopt-lock-narrow for
candidate 3; both cross-linked there).

**Candidate 1 — #1830 `wrkslots-forked-log-journal-no-exit`:**
recover-tail-event cannot run while any journal or seal is
outstanding, and recover cannot replay.** Description: §4.1
(the four resting shapes, both traces, verb-by-verb refusal
chain). Design: suffix set-aside in recover-tail-event under
the existing dry-run/echo/receipt discipline when outstanding
journals provably postdate or belong to the fork; plus a
replay-free dead-seal retirement. Acceptance: deterministic
tests reproducing fork+seal and buried-fork+finish states
(the model traces transcribe directly), each repaired by
sanctioned verbs only, the set-aside receipt naming every
event and journal it relied on. Notes: found by #1828's
model; requires a fork source today (see #1807), filed as
defence in depth so the repair exists before a second source.

**Candidate 2 — #1831 `wrkslots-create-recovery-ttl-drift-loop`:**
a config heartbeat-TTL edit strands create recovery in a
refusal loop.** Description: §4.2 (trace, the three refusals,
the unnamed config-revert exit). Design: the journal's
recorded TTL governs recovery/abort exactly as recorded hooks
already do; drift becomes an audit note; quarantine
classification must not die on the same `StateError`.
Acceptance: create crash, edit TTL, recover completes and
abort completes (each with the drift noted), quarantine still
available for genuinely stranded journals; eden twin
(cli.py:41541) covered. Notes: #1828 model, candidate C2.

**Candidate 3 — #1832 `wrkslots-prevalidated-starvation-under-churn`** (cross-linking #1276 adopt-lock-narrow, #1808, and #1829 stage 1):
whole-registry identity retries can be starved by unrelated
writes.** Description: §6. Design: document/implement a
jittered whole-command retry for `_RegistryBusy` callers, or
wait briefly for a stable identity before the final round;
no persisted-format change. Acceptance: a churn harness
shows adopt/create completing within a bounded number of
whole-command attempts at a stated heartbeat rate. Notes:
liveness only; the busy refusal itself is correct behaviour.

**Candidate 4 — #1833 `wrkslots-reopen-coordinator-reassert`:**
transfer-owner --reopen never re-asserts its captured
coordinator under the lock.** Description: §7. Design: mirror
the sibling captures (`_assert_caller_process` under the
mutation locks). Acceptance: a test with a coordinator
generation change between capture and commit refuses.
Notes: code-read during #1828 transcription, evidence hygiene.

## 9. Limits

The transcription risk is the main one: actors compress real
commands (phase tables in TRANSITIONMODEL.md), and a misread
guard changes results — mitigated by the two production
oracles reproducing exactly, by the discriminating controls
(§2), and by every refusal cited above being re-read in code
before it was reported (two model bugs found this way during
development were transcription gaps, now regression-tested:
the `_begin_finish` hold gate and the snapshot-shard heal).
Single machine; recover atomic; proof folded to three values;
Eden, archive-chain independence, lock timeouts, and
durations unmodelled (full list in TRANSITIONMODEL.md).
State counts are for the stated bounds; "unreachable" means
unreachable there, with the bounds chosen so both production
defects reproduce inside them.

