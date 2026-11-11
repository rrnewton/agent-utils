---
title: 'create-lock-hold-classification-cost: an agent create held the run-population lock 127 s, classifying 76,008 events per pass'
status: open
priority: 2
issue_type: bug
created_at: 2026-11-09T11:00:09.000000000+00:00
updated_at: 2026-11-09T11:00:09.000000000+00:00
---

# Description

On 2026-11-09 at about 10:45Z on another host, one wrkslots create --slot-type
agent held the project-root (run-population) lock for 127 s. Each classification
pass read the whole event log: 76,008 events, about 19 s per pass. A validate-run
waiting behind it then refused on a config-change race, and that host failed its
health check.

There is no existing issue for event-log compaction or a classification cache.
#613 is the full-text index, which is unrelated.

# Design

Measure first:
- Where do the 127 s go: event classification, image work (slot-image
  provisioning), or the lock itself (waits, re-reads)?
- How many classification passes does one create make, and which ones run under
  the lock?
- Does #1108's shape (do the expensive reads before taking the locks, keep only
  the re-checks under them) apply to create?

Then propose one or both of:
- event-log compaction: a snapshot plus a tail, with readers accepting both;
  follow the migration rule for layout changes;
- an incremental classification cache keyed by the event offset (and a log
  identity), so a pass reads only the new tail; invalidated safely on any
  rewrite.

Keep every safety decision on fresh evidence. A cache may only skip re-reading
events it has already classified, never re-judging liveness.

# Acceptance Criteria

- A measurement of the create lock hold split by phase, on a registry of tens of
  thousands of events (a fixture is fine), recorded in Notes.
- A design for compaction and/or the cache, approved by the lead before
  implementation.
- After the fix, create's lock hold no longer grows with the event-log length.

# Notes

Reported 2026-11-09 by the lead coordinator from a consumer's host. Filed by the
devbig030 coordinator. Related: #1108 remove-lock-hold-starves-validate.

Measurement (mu-createlock, 2026-11-09, devbig030, Python edition at 80c4bf2e,
316 CPUs, gate idle, load average 41-62). Fixture: a real `wrkslots init` plus
8 real agent creates (worktree representation, nested layout, one-file
repository), then the hash chain extended with chain-faithful generated events
(30% `active-state-recorded` ttl-renewed, about 59%
`operation-progress-recorded`, about 10% `operation-completed`; no
`archive-state-recorded`, 4.3% of the local real log, so the fold is slightly
understated). At 76,010 events the fixture log is 153 MB, mean 2,010 bytes per
event-file (local real log: 2,728). The chain was verified by the loader, fold,
pending scan, and `status` after every fill. Each measured item is one real
fresh-process `create --slot-type agent` (nested, one checkout), instrumented
around `_mutation_locks`, the event loaders, folds, pending scans, fetch,
worktree add, image provisioning, and hooks. Event files were settled (older
than the memo's 2 s change-time window) at each run.

Lock hold (the single `_mutation_locks` interval; lock wait 0.001 s,
uncontended) grows linearly with the event count:

| events before | hold | wall |
|---|---|---|
| 41 | 0.76 s | 0.81 s |
| 19,002 | 7.08 s | 7.15 s |
| 38,004 | 15.63 s | 15.79 s |
| 76,010 | 28.80 s | 29.09 s |
| 76,015 (repeat) | 29.00 s | 29.29 s |

Pass counts are constant at every size: 19 logical `_load_events`, 9
`_states_from_events` folds (events folded `[N, 0, 0, 0, 0, 0, 3, 2, 0]`), 5
`_pending_operations_from_events` scans, 24 event-directory listings, and
N+~10 event-file parses. The in-process memo (per-file identities, verified
chain, replay prefixes) works as designed: one pass parses and hashes every
file, one fold replays every event, and nothing is re-parsed. The cost is
repetition, not re-reading. Exclusive time at 76,010 events, of the 28.80 s
hold: first cold verify (open, parse, SHA-256 all 76k files) 5.5 s; first cold
fold about 1.2 s; seven later `_verify_event_paths` per-file memo walks 6.8 s
(0.6-1.1 s each — after the first journal append forgets the verified chain,
the append tail is always inside the 2 s window, so the chain never re-forms
and every later load lstats and re-walks all 76k files); verified-chain
identity lstat walks inside `_load_events` 4.9 s (about 0.22 s per hit);
directory listings 1.7 s (24 at about 72 ms); pending-operation scans 5.0 s
(5 at about 1.2 s, unmemoized pure-Python scans); the rest is journal/state
writes plus non-event work: storage census 0.21 s, plan plus local fetch
0.26 s (fetch alone 0.13 s against a local-path remote), `git worktree add`
0.06-0.09 s (one-file repository; scales with repository size), slot-image
provisioning 0.36 s (separate image run, hold 30.30 s), checkout verification
0.04 s. All of it runs inside the one hold: `_cmd_create` takes
`_mutation_locks` (project-root config flock, `ACTIVE.global.lock`,
`ACTIVE.<machine>.json.lock`) before any registry state is read and releases
only after publication (cli.py:29486-29754), so classification twice, fetch,
image, worktree adds, and hooks are all in the critical section. A refused
create pays too: one fixture-fault refusal at 76k held the locks 13.5 s.

Hooks add to the hold one-for-one: in a second fixture whose only
post-provision hook is `sleep 10`, one create at 15 events held the locks
10.70 s, of which `_run_post_provision_hooks` was 10.01 s.

Priming the memo before the locks (the #1108 shape, simulated in the harness
by wrapping `main()` in a pre-warmed memo scope whose results the command
never trusts) moves only the cold verify and fold out: hold 22.4 s at 76,025
events instead of 28.8 s. The remaining 22 s is the repeated warm walks, so
warm-up alone does not fix the growth; the number of loads and scans inside
the hold must also fall.

Cold readers at 76,020 events (fresh processes, zero journals):
`classify-create-journals` 14.9 s wall, `status` 11.7 s. The incident's
"about 19 s per classification pass" matches this cold-process scale on a
loaded host; inside one create process the repeat passes measured here are
lstat/memo walks, not re-parses. The incident's 127 s is consistent with
(inference, not measured) this event part scaled by host load, plus a
dependency-generator hook in the tens of seconds, a network fetch, 77 active
slots (storage census, per-fold path checks, mount activation; 12 active and
none archived here), and archive events. Multi-machine shards multiply the
per-machine loads; not measured.

Does #1108's shape apply? Yes, three ways. (1) Fetch is exactly #1108:
`git fetch` before the locks, ref and authority re-verified under them.
(2) The cold parse/hash/fold is a pure function of a verified prefix (the
replay memo already encodes this; the only filesystem-dependent part,
`_assert_record_paths`, re-runs for still-active rows every fold), so it can
be primed unlocked while trusting nothing. (3) Provisioning is #894/#1108
inverted: the expensive mutation of a new, exclusively claimed slot tree
(image, worktree adds, hooks) can run unlocked between a short reservation
and a short record, with re-verification under the record lock. Two things
do not move: liveness (re-judged fresh on every classification) and event
sequence allocation (appends still serialize under the mutation locks).

Design (for the lead's approval; implementation after #1121 and #1122 land,
which edit the same journal/classification and hook code):

- Unlocked phase: fetch per repository, preliminary plan, memo warm-up.
- Reservation (locks, short): re-verify config, global state, interrupted
  creates unrelated, slot/agent/branch free, start points still the fetched
  refs, disk floor, target clear; write the CREATE journal naming the
  creating process's own identity, and the first progress event. The journal
  claims slot, agent, branches, and paths exactly as an interrupted create
  does today.
- Provisioning (unlocked): image/subvolume, worktree adds, hooks, git
  identity. Each journal progress rewrite re-takes the locks only to append
  its progress event (append-sized holds), first asserting the journal's
  operation id and creator identity are still this process.
- Record (locks, short): re-verify journal ownership and completeness,
  checkout identities unchanged, create processes live; publish the ACTIVE
  row and completion event; the only-this-slot-changed check must tolerate
  disjoint rows other creates added meanwhile.
- Busy semantics (the #1121 lesson): a CREATE journal whose creator is live
  is provisioning. Disjoint classifiers stay nonblocking; a related waiter
  (same slot, agent, branch, or path) gets `_JournalBusy` (exit 75) instead
  of an interrupted-create refusal; creator dead means interrupted and
  recovers as today. The owner/coordinator agreement refusal must not fire
  while the creator is live, and the assumption in
  `_create_journal_owner_state` that a mutating classifier cannot see an
  in-flight create is retired. Whoever resumes a journal rewrites the
  creator to itself, as #1121 does for finish. Phase 2 also adds
  creator-dead to `quarantine-journal` eligibility (#1086, landed during
  this measurement), which already requires proven-dead owners and 24 h
  age, so a live provisioning journal stays ineligible.
- Journal format: recommend landing #1124 journal-reader-extension-slot
  first and putting the creator identity in the extension slot, rather than
  accepting a #1121-style flag day. A finish journal is outstanding for the
  short deletion window; a provisioning CREATE journal is outstanding for
  the whole image/hook phase and after any crash until recovery, on a
  registry shared by older pins that would StateError on the new key in
  exactly that window. Rollout: reader pins reach #1124 before writers
  enable out-of-lock provisioning; post-#1124 readers ignore the creator
  and fall back to today's verdict. The alternative (accept the flag day
  with lockstep pin upgrade in the upgrade notes) is the lead's to choose.
  #1124 needs both editions and differential tests.
- Event-cost reductions with no format change (land with or before the
  above): memoize `_pending_operations_from_events` by (count, tip) like
  replays; one shared `_EventWriter` per create instead of a full
  `_load_events` per append (a writer needs only count and tip); one
  `_states_from_events` per machine in `_validate_global_state_once`
  instead of entering it once for active and once for archive; skip
  create's second classification when the event tip and journal set are
  identity-unchanged since the first.
- Persistent incremental classification cache (after the lock work):
  one advisory file per machine shard beside ACTIVE, keyed by log identity
  (event directory, count N, tip SHA-256), holding the replay fold at N,
  the pending-operations map at N, and the per-file identity vector the
  in-process memo already trusts (settled files only, same 2 s window). A
  cold reader verifies identities, reads and verifies only changed or
  post-N files, checks the tail hash-links onto the cached tip, and folds
  only the tail; any mismatch falls back to today's full verify, and
  doctor keeps a full-verify path. Writers refresh it under the locks when
  stale past a small bound; pure readers never write it. It caches only
  facts derived from already-classified events, never liveness, process
  state, git observations, or journal stability.
- Compaction (snapshot plus tail) only if post-cache directory listing and
  inode counts still hurt: an explicit `wrkslots compact` (dry-run by
  default) writes the snapshot under the locks after a full verify, moves
  files 1..N into a per-machine archive collection in its own directory
  (never inside `EVENTS.<machine>/`), and keeps absolute sequence names
  for the tail. Readers accept both layouts from day one, per the
  growing-collection migration rule; both editions must read it
  (differential tests), and archived segments stay available for offline
  full-history verification. Recommendation: cache first, compaction if
  measurements still demand it.

Projected effect from the numbers above: reservation plus record at 76k
with one writer, memoized pending, and a primed memo is a few seconds of
hold instead of 28.8 s, independent of hooks and fetch; with the persistent
cache the classification part reads only the tail, so the hold stops
growing with the log. Tests for the implementation: a sleeping hook no
longer blocks a disjoint create or classification (stated lock-wait bound);
a related waiter gets busy (75) naming the creator; creator killed mid-hook
classifies interrupted and recovers as today; legacy journals without a
creator classify exactly as today; hook failure mid-provision leaves
today's journal and recovery behavior.

## Update 2026-11-11 (#1287 retire-lock-narrow)

[watermelon-full-preview] Coverage of this issue after #1276 and #1287, measured with the counted lock ledger of `py/wrkslots/tests/test_prevalidated_mutation.py` plus plan/fetch/hook instrumentation on a fixture create (2 existing rows, one trivial post-provision hook):

- (1) The 2026-11-09 incident path -- classification passes folding 76,008 events per pass under the lock -- is the path #1276 fixed: a plain create now runs its fleet validation, slot/agent check and fleet storage report lock-free (each counted once, never held), takes one exclusive round, and under it reads only the verified tip plus the events it wrote (9 event reads for this fixture's create), with no event-log load or listing held. Create's lock hold on this path no longer grows with the event-log length beyond the registry-identity lstat scan #1276 already accepted.
- (2) #1287: a create beside an outstanding journal previously deferred to the locked path and recomputed the fleet storage report under the flock; every round now computes that report lock-free and the locked classification path reuses it (`py/wrkslots/tests/test_create_lock_scope.py`). The journal classification itself still runs under the lock, as its recovery semantics require.
- Still open here, unchanged by #1287: inside create's one locked round sit `_create_plan` (including `fetch_remote`), `git worktree add`, and `_run_post_provision_hooks` -- each counted held exactly once in the ledger above. On a host whose provision hook runs a multi-GB `git submodule update --init`, that hook time is spent under the root flock (two creates measured at 2 min 9 s and 2 min 35 s on 2026-11-11). Moving fetch and hooks out is the reservation/provisioning/record design above; it remains unimplemented, and the persistent incremental classification cache and compaction proposals remain deferred pending lead approval.
