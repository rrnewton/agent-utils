---
title: "census-single-pass: the process census walks /proc once per slot instead of once per command"
status: closed
priority: 2
issue_type: bug
assignee: cl-census
created_at: 2026-11-10T00:40:00.000000000+00:00
updated_at: 2026-11-10T19:30:00.000000000+00:00
---

# Description

The wrkslots process/path census walks /proc once for each slot it checks. A consumer measured ~12 s per slot,
~22 minutes for 111 slots at 4,255 processes (2026-11-09). A single pass over /proc that maps each process's
references (cwd, root, exe, fds, maps, and the same-UID and privileged census results) to the slots they fall in
takes about 1 s.

# Design

Build one census per command: walk /proc once, collect each process's references with the same readers and the
same error classification as #1173 (ESRCH/ENOENT = gone; EACCES and others = indeterminate), and index them by path
prefix and (dev, ino) so each slot's question is a lookup. Keep any per-slot follow-up (lsof fallback, fenced
scans after a fence) as it is, since those are deliberately after-the-fact. #1174 (the Rust census reads root and
maps) lands first, so both editions read the same entries.

# Acceptance Criteria

- One /proc pass per command for remove, remove-agent-batch, audit and doctor/status, in both editions.
- Results identical to today's per-slot census: a differential test over a fixture with slots used by cwd, root,
  exe, fd and mmap holders, plus a vanishing process.
- A measurement at 100+ slots and a few thousand processes (synthetic), before and after, recorded in Notes.
- Vanished processes handled as in #1173.

# Notes

Requested by the lead coordinator (2026-11-09) from a consumer's measurement. For cl-census, after #1173 and #1174.
Related: #1172 audit-lock-hold (its row checks are a separate cost).

## Resolution

Delivered as the lead's option (A) (2026-11-10, cl-census, opus 5.5). (B) is #1201 batch-grouped-fencing and the
clean-caches walk is #1202.

Where the per-slot /proc walks were: Python `remove-agent-batch` (single agent `remove` is a batch of one) walked the
whole host once per item before its locks (`observe_unlocked` -> `_agent_batch_fresh_use`), so N items were N walks
plus the per-item fenced scans after each fence. `audit` was already one census per command in both editions
(Python `_capture_process_path_census`, Rust's memoized snapshot). `doctor`/`status` make no host-wide /proc walk
(only per-owner /proc/PID reads). Rust `remove` handles one slot (one snapshot before the fence, one after), and
Rust has no batch command, so there is no Rust change.

Change: `_agent_batch_fresh_uses(targets)` walks /proc once for every target, with the same readers and order as
`_agent_batch_fresh_use`: ownership, cgroup, mount table, cwd, root, exe, descriptors and mappings, surviving
threads of an exited leader, and one relisting. Each process is read once, and a target stops at its first use or
refusal, as the per-slot scan does. A read that would raise in the per-slot scan becomes that refusal for the
targets still collecting. The cgroup is read for every process, because whether it is evidence depends on the
record. `_AgentBatchFreshCensus.lookup(slot, record, capture_generation)` replays the walk in order and returns or
raises exactly what the per-slot scan would. `_AgentRemoveBatchContext.capture` takes the walk right after the
shared lsof census, under the scan bound; a failure leaves `fresh` None, and each item then scans as before. The
walk is repeated with lsof when older than 300 s. `observe_unlocked` answers each target from it once per capture;
a second check after waiting for a holder to exit scans that slot afresh, so it cannot loop on stale evidence. The
fenced scans after each fence are unchanged. Vanished processes are handled as in #1173 (gone reads are skipped and
noted).

Tests: `test_one_shared_walk_answers_every_slot_as_its_own_walk` (test_proc_status.py) runs real children using
six slots by cwd, root (chroot in a user namespace), exe, fd and mmap, an unused slot, and an exited process, over
a proc root listing exactly them. For every slot, with no record and with a record whose owner cgroup is evidence,
in both generation modes, the shared lookup equals the slot's own scan. Walk counts (test_lifecycle.py): a
three-item batch makes `fresh_process_scans == 1` (was 3); an agent `remove` makes one `_agent_batch_fresh_uses`
and no `_agent_batch_fresh_use`, and no scan under the registry locks.

Measurement, synthetic /proc (no real processes; generator and script under /tmp/cl-1175/): 3,000 processes (8
fds, 40 mapped libraries, 4 distinct mount tables each), 120 slots, 31 of them in use by cwd, root, exe, fd or
map. Before: 120 per-slot walks, 74.1 s (0.62 s per slot). After: one shared walk 3.45 s plus 120 lookups, 3.48 s
in all (21x), with identical verdicts for every slot.
