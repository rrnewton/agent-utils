---
title: 'dagrun-anon-memory-peak: dagrun records each step''s anon and shmem memory peaks beside its cache-inclusive peak'
status: closed
priority: 2
issue_type: feature
labels:
- dagrun
- memory
- perf-log
created_at: 2026-11-25T19:35:00-05:00
updated_at: 2026-11-26T07:50:00-05:00
closed_at: 2026-11-26T07:50:00-05:00
---

# Description

The owner's rule (2026-11-25) is that step memory limits keep 50% or more headroom over what the step really needs. Sizing that needs each step's program-memory peak, and dagrun records only `peak_bytes`, the step cgroup's `memory.peak`. That counter includes page cache, which the kernel reclaims under the cap instead of killing the step, so several steps read as pinned at their cap with no OOM kill: the cap looks exhausted while the program itself used much less.

cgroup v2 has no high-water counter for anonymous memory or shmem. `memory.stat` gives their current values (`anon`, `shmem`), so a peak has to be sampled. shmem matters beside anon because tmpfs pages cannot be reclaimed without swap, so they count as demand the same way anon does.

# Design

- `rs/dagrun/src/cgroup.rs`: a `CgroupManager` method that reads the step cgroup's `memory.stat` and returns its `anon` and `shmem` values in bytes. It defaults to `None` (not measured), and a missing or unreadable file, or a counter that is absent or does not parse, is `None` too, never 0.
- `rs/dagrun/src/scheduler.rs`: the existing per-step monitor thread takes one reading per tick when the step is boxed and keeps the running maximum, plus one final reading after the step exits and before its cgroup is removed. No new thread; constant work per sample.
- Perf log (`rs/dagrun/src/perflog.rs`, `py/dagrun/perflog.py`): new columns APPENDED at the end of the step-profile row, so readers that go by position keep working, with the Rust and Python schemas kept the same. The values are sampled maxima, so they are lower bounds on the true peaks, not kernel high-water marks; the column names and docs say so, and the sampling interval is recorded beside them. A missing reading is a blank field, never 0.
- No change to sizing or admission in this issue: consumers (learned caps, estimates, reports) can read the new columns in follow-ups.

# Acceptance Criteria

- A boxed step's perf-log row carries its sampled anon and shmem maxima in bytes beside `peak_bytes`; an unboxed step, or one with no readable sample, has blank fields, never 0.
- The new columns come after every existing column in both the Rust and the Python schema, in the same order, and every perf-log reader in the repository still reads old and new logs.
- Tests that fail without the change: `memory.stat` parsing (present, missing file, missing key, malformed value), the running maximum (never invents 0), the final sample at exit, and Rust/Python column parity.
- `cargo fmt` and `cargo clippy` clean for `rs/dagrun`.
- The persisted format change (new columns, their order and meaning) is reported to the consumers that read the perf log before it lands.

# Notes

Drafted by a consumer team that sizes step memory limits from the perf log, then reviewed, adapted and landed here.

## Resolution

Landed as a22752db (the seven #2271 commits, replayed onto main) by cl-anonpeak on 2026-11-26.

- A boxed step's perf-log row now carries `anon_sampled_max_bytes`, `shmem_sampled_max_bytes` and `mem_sample_interval_ms`, appended after every existing column in the same order in both engines. They are blank when nothing was measured (unboxed step, unreadable `memory.stat`, or a manager without the sample), never 0. The step monitor samples `memory.stat` on every tick it takes: the 1 s guard tick, or the shorter `--profile-timeseries` tick.
- Review fixes on top of the draft:
  - The final sample is taken after the step's tree is reaped. It used to fold the dead tree's anon (already freed) into the maximum. Now a reading taken after the step exits keeps only shmem, which tmpfs keeps charged. A step shorter than one tick records its shmem and a blank anon.
  - `memory.stat` values parse strictly in both engines: ASCII decimal only, within i64, keys matched whole, first occurrence wins.
  - The Python engine gained the same sampling. The sample method is optional, so older managers still work.
  - Added tests that fail without these fixes. A column-parity test reads the Rust column list.
- The format change was reported to the consumer that reads the perf log, and approved before landing. It reads columns only by header name.
- Validation: gate run 1795695216315007871-4c6f8d0e on f8538c7d, full selection, 62 steps, exit 0, no failed steps, preflight passed. Verdict PARTIAL: 7 cross checks were skipped as delegated live scope (5 in the cpuset differential, 2 in the dagrun differential). Those are systemd cases that cannot run inside the gate's cgroup, allowed by design and tracked in #28. Every selected step passed.
- An earlier gate on 689ab7bd was refused at preflight because two new public docs named the other engine. f8538c7d fixed that.
