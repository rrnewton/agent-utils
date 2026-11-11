---
title: 'wrkslots-status-lock: a read-only status holds the global state lock across a long scan'
status: closed
priority: 1
issue_type: bug
labels:
- wrkslots
- state-lock
created_at: 2026-11-10T21:27:58+00:00
updated_at: 2026-11-11T02:38:54+00:00
closed_at: 2026-11-11T02:38:54+00:00
---

# Description

[opus 5] P1 defect report from a consumer lead (2026-11-10, about 4:24 PM ET), live on the shared host now. `wrkslots --wait-lock 600 status`, run by a reviewer lane at about 3:43 PM ET, held the `ACTIVE.global.lock` file (fd 3) for about 40 minutes in state R at full CPU. Every wrkslots mutation on the host was refused (`state lock is busy for .../worktrees/ACTIVE`), including slot creates. The hourly audit service failed (exit 2), the observer service timed out, and a health tick timed out at 651 s. A read-only `status` must not hold the global state lock across a long scan, or the scan must be bounded. The report notes the slot table reported `retained_storage_inconsistencies=9`, which may be what makes the scan slow; that is a hypothesis to test, not an established cause.

Related: #786 agentctl-start-slot-bind-busy-lock (a `status` read refused on a busy lock), #1172 audit-lock-hold (phased readers: snapshot under a short lock, measure outside it), #1194 wrkslots-lock-scope (salvage and census before the configuration lock), #1108 remove-lock-hold-starves-validate, #1213 wrkslots-state-lock-deletion (slow removal phases outside the ACTIVE locks). None of those covers a read-only scan spinning under the lock or a wall bound on read-only scans.

# Design

1. Root cause first: why `status` takes 40+ minutes at full CPU. Suspects: a scan that is quadratic or worse in inconsistency findings, slots, Git worktree registrations or processes; a census retry loop; repeated Git subprocesses per finding. Reproduce on a local registry (which also reports dozens of retained-storage inconsistencies and a slow `status`) and record where the time goes before changing anything.
2. Lock scope: read-only commands (`status`, `doctor`, `audit`, `unpushed`) never hold the global state lock across a scan. They snapshot the state under a short lock (well under 1 s), then scan unlocked; anything they report comes from that snapshot.
3. Wall bound: a bound on read-only scans (default a few minutes, configurable), after which the command reports partial results with a clear marker and exits non-zero instead of running on.
4. Speed: fix the slowness itself where the root cause allows (for example cache per-repository Git worktree lists, skip repeated census work).

# Acceptance Criteria

- Root cause recorded in Notes with before/after measurements (operation or subprocess counts, never wall time alone).
- A lock-contention test: a `status` scan in progress does not block a mutation beyond the snapshot window.
- A large-fixture test of the algorithmic fix that counts operations or subprocess calls and never asserts on wall time.
- `status`, `doctor`, `audit` and `unpushed` each snapshot under a short lock and scan unlocked; a scan past its wall bound reports partial results with a clear marker and a non-zero exit.
- Existing lifecycle, audit, census and lock-scope tests pass.

# Notes

Report evidence only at filing time; no measurement had been made yet. The spinning process had exited before this lane could read its stack, so the root cause was reproduced on a local registry.

## Root cause (mu-statuslock, 2026-11-10)

Measured on the local registry (35 slots, 1,595 events, 75 Git worktree registrations) with cProfile and operation counts, plus a synthetic-fixture scaling probe (10/40/160 slots, 0-640 stale registrations):

- The storage scan is linear, not quadratic: Git calls are exactly slots+2 (one `worktree_identity` probe per checkout; the per-repository common-directory and worktree-list calls are cached across slots sharing a repository), and stale registrations add findings without adding Git calls (160 slots with 640 stale registrations: same 322 subprocesses, same wall as without). The report's hypothesis that the inconsistency count drives the slowness does not hold.
- The scan's real cost is per-slot subprocesses: one liveness command per agent slot plus one Git identity probe per checkout (locally 72 subprocesses, 2.4 s of a 3.3 s `status`). The legacy per-agent liveness spelling ran with no timeout at all, so one hung or slow probe stalled the whole scan behind it; on a loaded host with 100+ slots this alone reaches minutes.
- Lock scope made that a host-wide outage. `unpushed` held the shared registry lock across every per-checkout Git probe and remote fetch by construction. `status` and `doctor` measure outside the lock, but when the registry's content identity changed during a measurement (any heartbeat renewal or event append changes it), then after two retakes the last measurement ran under the shared lock again. On a busy registry a reader therefore ended up measuring under the lock for the whole scan while every mutation waited. No read-only scan had any wall bound, so nothing ever gave up.
- One genuinely quadratic scan fed the same commands: `_handoff_write_publication` rescanned the whole event log for every handoff-write intent (via `_owner_before_handoff_write_intent`), with a full record parse per historical event. Every `status`, `doctor` and handoff check of a released slot paid intents x events parses.

Fix: measurements never run under the lock (the latest snapshot's measurement is published unconfirmed after the retakes; it still describes one snapshot instant); `unpushed` snapshots under a short lock and scans outside it; every read-only scan takes `--scan-timeout` (default 180 s) and past the bound prints partial results marked PARTIAL (`"partial": true` in JSON) and exits 2; one liveness call is bounded at 15 s; handoff publication is a single pass parsing each historical record once. Tests count Git calls and record parses, never wall time.

Memory (P0 follow-up, same day): a plain `status` on the 185-slot registry (78,892 events, 429 MB log) reached 640 GiB RSS and was OOM-killed. Root cause of the blowup (found by the consumer's lane, confirmed here): `_registry_read_identity` hashed every regular file in the control directory except `*.lock` with `read_bytes()`. On that registry the control directory holds the 640 GiB (apparent) btrfs worktrees image, so each identity call read the whole image into memory; `_phased_registry_read` calls it at least twice per run, the first time under the shared lock, which is also the 40-minute lock hold. The fix hashes only registry files (ACTIVE.*, ARCHIVED.*, *.journal, HOLD.*, HANDOFF.*, the event listing, quarantine journals, slot-image records) and never opens the configured worktrees image or any other unnamed file. Loading alone on a copy of that control state measured 78,892 events parsed in 29.7 s at 691 MB tracemalloc peak / 1.73 GiB RSS, before any Git or liveness work: the per-file memo and the kept verified chain retained the whole parsed history for the command's life, and retakes re-verified it (1.3 TB of reads). Amendment: retained parsed-event bytes are capped (256 MiB of event files; no whole chain over 5,000 events; past the cap the memo starts over and later loads re-read), a streaming verifier (`_iter_events`) folds record by record retaining nothing, and read-only scans cap their own address space with `--scan-memory-limit` (default 4 GiB, RLIMIT_AS), past which they report PARTIAL and exit 2. Full streaming folds were not folded into the replay path: the parse-once/fold-once memo contract is pinned by `test_event_memo.py` (one parse and one fold per event across a whole batch command), so retention is bounded by the cap plus the hard process limit instead. The same loader serves `adopt`/`create` under their mutation locks, so the cap also bounds their load retention there.

## Resolution

Landed on main as 2473d23b (mu-statuslock). Measurements never run under the registry lock; `unpushed` snapshots under a short lock and scans outside it; all four readers take `--scan-timeout` (180 s; audit 600 s) and `--scan-memory-limit` (4 GiB, RLIMIT_AS), reporting PARTIAL with exit 2 past either bound; one liveness call is bounded at 15 s; handoff publication is single-pass; retained parsed-event bytes are capped at 256 MiB with no whole chain over 5,000 events; `_registry_read_identity` hashes only registry files and never opens the worktrees image. Gate 3c37a169 ran the full contract on the combined tree: the only reds were two lifecycle census/fence tests that pass solo and touch none of the changed paths. Not verified on the consumer's live registry (never run there); the control-copy status peaks at 0.76 GiB RSS.
