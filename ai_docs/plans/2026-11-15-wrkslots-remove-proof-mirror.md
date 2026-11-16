# wrkslots remove proof mirror: design note

Status: design only (2026-11-15). Implementation waits for the
coordinator's go. Governs #1692 `wrkslots-remove-proof-mirror`,
follow-up to #1681 `wrkslots-remove-proof-fetch-cost` parts 1-2.

## Problem

A removal must prove, before deleting the only copy of a commit,
that the commit is already durable elsewhere. The witness is the
remote's advertised refs: one `ls-remote` per repository, then one
batched commits-only fetch (`fetch --stdin`, `--filter=tree:0`)
of the advertised tips missing locally into an empty proof store.
Parts 1-2 of #1681 made that once per repository per removal and
revalidatable by advertised-inventory fingerprint, but every
removal still does its own network work, and under a queue of
dozens of slots the same remote is asked the same questions
dozens of times.

## Design

One host-wide mirror per remote URL, owned by the coordinator,
in its own collection directory under the wrkslots control area
(one entry per remote, keyed by URL digest; a growing collection
gets its own directory, per the repository layout rule).

- The proof step reads evidence from the mirror when the
  mirror's last successful fetch is within the documented
  freshness bound (300 s, the same bound removal evidence
  already uses). The fetch timestamp is part of the evidence
  and is recorded in the salvage receipt.
- When the mirror is stale, missing, or fails its own
  consistency check, exactly one fetcher refreshes it: an
  incremental `git fetch` into the mirror under a per-mirror
  flock, recording the new fetch time only after the fetch and
  a mirror self-check pass. Removals that find a refresh in
  progress wait briefly or fall back to a direct proof against
  the remote (today's behaviour); they never trust evidence
  older than the bound.
- Removals only read the mirror: advertised refs from its ref
  snapshot, objects through alternates. Concurrent removals do
  not serialize on the mirror; only refresh does.
- The mirror is an evidence cache, never the only copy of
  anything. A mirror that fails verification is discarded and
  refetched; nothing in a slot's salvage path writes to it
  except the designated fetcher.

## Safety trade-off

Mirror evidence is the remote as of its last fetch, not as of
this moment. A branch force-pushed or deleted inside the
freshness window would make the mirror over-claim durability.
The bound keeps this no wider than today: a single removal
already trusts evidence up to 300 s old within itself; the
mirror only lets several removals share one observation inside
a window one removal already accepts. Any weakening of that
bound, or trusting the mirror when its fetch record is absent
or unverifiable, is out of scope and must fail closed to the
direct proof.

## Cost

One full object store per distinct remote on the host, plus one
incremental fetch per freshness window per remote, regardless
of queue length. Per removal: no network work of its own in
the common case; the remaining time is local (bundle creation
and verification, all-objects history scans), which #1681
parts 1-2 already reduced to once per removal.

## Coordinator decisions (2026-11-15)

- Storage: a commits-only mirror (`--filter=tree:0`, the shape
  of today's proof store; proof needs commit reachability
  only), rooted in a `proof-mirrors/` collection directory in
  wrkslots' per-host state area, outside every Git working
  tree, on the slots' filesystem. It is a regenerable cache:
  `clean-caches` may delete it at any time except during a
  removal that holds its lock, and `doctor` reports its size.
- Refresh: lazy only (on proof demand) for the first step. No
  periodic tick.
- Retirement: `clean-caches` drops a mirror idle for 7 days
  or with no live slot on that remote.
- Freshness evidence: the ref snapshot and the fetch come
  from the same refresh — `ls-remote` under the per-mirror
  lock, then fetch exactly those tips. The receipt records
  both times and the inventory fingerprint. A removal compares
  the remote's current fingerprint (one ls-remote, as part 2
  does) only when past the bound; within the bound the
  snapshot stands.
- Fail closed to today's direct proof on any doubt: missing
  record, failed self-check, lock timeout, URL mismatch.
- Tests: two concurrent removals share one refresh; a stale
  mirror falls back; a force-push inside the window is the
  documented trade-off (and is caught past the bound); a
  corrupt mirror is discarded. Condition waits only.
