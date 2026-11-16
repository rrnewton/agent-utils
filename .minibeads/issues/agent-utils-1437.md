---
title: 'wrkslots-remove-salvage-lock-hold: do the submodule private-history scan before the configuration lock'
status: closed
priority: 1
issue_type: bug
labels:
- wrkslots
created_at: 2026-11-16T14:00:00+00:00
updated_at: 2026-11-16T14:00:00+00:00
---

# Description

On a fresh slot from another team on the host, `wrkslots remove SLOT --no-lock-budget` held the configuration lock for 13 min 01 s, blocking every create and remove on the host; another lane's remove failed with "configuration lock is busy". With the default 300 s lock budget the same remove refuses outright. The time went to salvage's private-history scan of the slot's submodule checkouts: the output reported hundreds of dangling commits per submodule (667, 441 and 44 in three submodules, plus more in two others), each "not keeping N dangling commits, held by source store".

Real-run evidence from the host: the remove started, spent about 10 min 20 s scanning before its first salvage write, about 10 s writing, then about 2.5 min in post-fence and final rescans plus deletion, all under the lock. Scale per checkout: a superproject with 4284 refs (4165 remote-tracking), submodules with 97-837 refs, and 8 checkouts including nested submodules, each scanned before and after the fence and in the final pass.

Profiling on a synthetic fixture (fast-import histories of 40k/8k/2k commits, 100-4000 remote-tracking refs with reflogs, 200-700 dangling commits a source store also holds, plus a few slot-only ones) showed one `_submodule_private_history` scan costing 0.4-1.5 s warm and idle, split across `_reflog_entries_by_ref` (parses every reflog file; scales with refs and entries), the every-commit enumeration (`cat-file --batch-all-objects` plus `log --no-walk`; scales with total history), `_commit_tips`, and `_held_by_source_store` (already one batch check per store, not the problem). The multiplier is repetition: a removal rescans each checkout several times, and the private-history scan was never memoized, so the full cost landed under the configuration lock on every pass.

# Design

- `_submodule_private_history` (py/wrkslots/cli.py) keeps its signature and return type. While one removal runs, its result is memoized by `_private_history_fingerprint`: the checkout's refs and configuration, reflog file state, loose-object names, pack files, the kept commit, every durable repository's refs, and the prune-expiry configuration. Under the lock only the fingerprint is rechecked; a changed checkout is rescanned in full, never trusted stale. Callers outside a removal scope (audit included) set no memo and always scan in full.
- `_pregather_salvage_facts` warms each declared submodule's private history before the configuration lock, alongside the salvage facts it already gathers there, so the locked removal normally finds the scan already done.
- Per-tip work shrinks where the answer cannot change: pack-object mtimes are read only for dangling tips no durable store holds (held tips are never kept), and the reflog-reachability check in private-state salvage walks `rev-list --reflog --all` once for all kept tips instead of once per tip.
- The salvage contract is unchanged: the same commits are kept, refused or bundled, the counts and receipts stay identical, and the fail-closed behaviour is unchanged. No salvage receipt, journal or archive-record key changes.
- Rust parity: `submodule_private_history` (rs/wrkslotsd/src/remove_write.rs) computed tips with one `merge-base` call per commit pair and passed every commit as a command-line argument; it now lists commits over stdin and drops every commit another listed commit names as a parent, in two `rev-list` passes, matching Python `_commit_tips`. The Rust edition still refuses every initialized-submodule checkout wholesale in `salvage_gate`, so it refuses where Python proceeds; that fail-closed edition gap is unchanged by this issue.

# Acceptance Criteria

- A removal's submodule private-history scan runs before the configuration lock; under the lock only the fingerprint is rechecked (observed by recording whether the lock is held during the scan's object enumeration).
- A fingerprint change during a removal forces a full rescan with the updated result.
- Within one removal, identical scans derive the facts once; outside a removal scope nothing is cached.
- Salvage receipts, counts and journals are identical to before for the same slot state.
- Rust tip computation agrees with Python `_commit_tips` on maximal commits.

# Notes

Follow-up candidate, out of scope here: `cat-file --batch-all-objects` also enumerates a repository's alternates (measured: 40,001 commits with an alternate to a 40k-commit store against 1 local commit, about 0.04 s per 40k objects warm). Narrowing the enumeration to the local object directory would change the dangling-held counts in the salvage receipt, so it needs its own issue and receipt analysis.

## Resolution

Delivered by mu-salvlock (devbig030), 2026-11-16.

- py/wrkslots/cli.py: `_submodule_private_history` is a memoizing wrapper over `_submodule_private_history_uncached` with an unchanged signature; `_private_history_fingerprint` reads refs, configuration, reflog file state, loose-object names, pack state, the kept commit, durable refs and prune configuration, with the fence rename folded back to one checkout identity. The memo (`_PRIVATE_HISTORY_MEMO`) lives only for one removal via `_remote_containment_scope`. `_pregather_salvage_facts` warms declared-submodule private history before the lock. Pack mtimes skip source-held tips; `_reflog_reached_tips` answers the reflog check for all kept tips in one walk.
- Rust: rs/wrkslotsd/src/remove_write.rs computes tips in two stdin `rev-list` passes instead of a quadratic `merge-base` loop; the now-unused argument-form helper was removed. The Rust edition has no held-by-source-store classification and refuses initialized-submodule checkouts wholesale (fail-closed), named here as an edition gap rather than changed.
- Tests: py/wrkslots/tests/test_lifecycle.py gains a cache/fingerprint-change/outside-scope test and a lock-state probe test showing the scan's object enumeration happens with the configuration lock free; rs/wrkslotsd gains a maximal-tips parity test. Existing private-history count, receipt and journal tests pass unchanged.
