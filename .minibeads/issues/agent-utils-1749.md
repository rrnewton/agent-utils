---
title: 'wrkslots-audit-systemerror-journal-cache-slots: audit crashes with SystemError under its scan memory bound instead of reporting partial results'
status: open
priority: 0
issue_type: bug
labels:
- wrkslots
created_at: 2026-11-16T22:50:00.000000000+00:00
updated_at: 2026-11-16T22:50:00.000000000+00:00
---

# Description

`wrkslots audit --format json` on a large registry (about 85,000
append-only events, 76 active rows) intermittently exits 1 with a
traceback ending in `SystemError: error return without exception set`,
raised inside registry validation on the way to the
`_all_journal_cache_slots(config, states=all_states)` comprehension in
`_cmd_audit_scan`. A scheduled audit unit therefore records no result,
and follow-on recovery that consumes audit JSON is blocked.

The scan memory bound (`--scan-memory-limit`, default 4.0 GiB) is an
address-space self-limit applied with `resource.setrlimit(RLIMIT_AS)`
in `_apply_scan_memory_limit`. Under that cap, a C-level allocation
inside registry validation (JSON/event parsing) can fail without
setting `MemoryError` on the observed CPython 3.12 build, so CPython
surfaces `SystemError: error return without exception set` instead.
`_cmd_audit` converts only `_ScanTimeout` and `MemoryError` into the
clean partial result (exit 2), so the `SystemError` form escapes as a
traceback with exit 1. The same registry with the bound disabled
completes; with a small bound (1 GiB) the `SystemError` reproduces
deterministically on both the pinned and the current implementation.

# Design

- Convert a failure under the bound into the existing partial result
  (exit 2) only when the scan memory limit is active and the error is
  the allocation-failure form: a `SystemError` whose message is
  `error return without exception set`. Any other `SystemError`
  keeps its traceback. The partial detail names the conversion so an
  operator can tell a bound hit from a wall-timeout hit.
- Apply the same narrow conversion in every read-only scan handler
  that installs the bound (`audit`, `unpushed`, `doctor`, `status`),
  beside each existing `MemoryError` partial path, including scan
  phases that run before the existing guarded section.
- A census or Git child killed under memory pressure already surfaces
  as a per-root error or a refusal, never as a traceback; keep that.
- Measure peak address space (VmPeak) and peak RSS of a full audit on
  a registry of this size, and size the default bound from the
  address-space peak with headroom. The `--help` text already names
  the limit as address space; keep that wording accurate.
- Check what the journal-cache pass holds in memory: event logs are
  loaded whole per machine and retained for the scan. Take a
  streaming reduction only if one is small and behaviour-preserving.

# Acceptance Criteria

- A hermetic regression test drives a read-only scan under a small
  memory bound (or an equivalent injected allocation-failure
  `SystemError`) and observes the partial result with exit 2 and a
  detail naming the memory-bound conversion; no traceback, no exit 1.
- A `SystemError` of any other form, or the allocation form with the
  bound disabled, still propagates as a traceback.
- The default `--scan-memory-limit` is sized from measured peak
  address space of a full audit on a large registry, with headroom,
  and the measurement is recorded in the Notes.
- Python and Rust editions agree where both implement the behaviour;
  the address-space bound itself is installed only by the Python
  command handlers, so the Rust reader needs no change unless it
  grows the same bound.

# Notes

Reproduced on a scratch copy of a large registry held outside every
worktree (read-only toward the source): a 1 GiB bound raises
`SystemError: error return without exception set` from
`_validate_global_state` in `_cmd_audit_scan` on both the pinned and
current code; the reporter's heavier run reached the same C-allocation
failure a few lines later, in the `_all_journal_cache_slots`
comprehension. With the bound disabled the same copy completes.
