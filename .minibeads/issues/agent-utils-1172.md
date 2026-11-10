---
title: "audit-lock-hold: an hourly wrkslots audit holds the registry lock for minutes at 100+ slots"
status: closed
priority: 1
issue_type: bug
created_at: 2026-11-09T23:35:00.000000000+00:00
updated_at: 2026-11-10T01:10:55.000000000+00:00
closed_at: 2026-11-10T01:10:55.000000000+00:00
---

# Description

A consumer runs `wrkslots --wait-lock 300 audit --format json --gate` hourly from a user timer. Measured
2026-11-09 by that consumer's profiling lane: at 105 slots one audit took 244.3 s wall and 116 CPU s, and was still
running when the sample ended. It contends for the registry lock: a slot `adopt` under `--wait-lock 600` waited
minutes behind it. #1108 remove-lock-hold-starves-validate and #1142 image-remove-fence-first fixed the same shape
for remove (slow checks under the registry locks); audit still has it.

# Design

1. Measure first: where audit spends its time (process census, git status, salvage and unpushed checks, image or
   mount checks, journal parsing), and how long it holds the registry lock, per slot and in total. Reuse the #1108
   lock-hold measurement approach (wrap `_locked` and the expensive helpers, print a timeline).
2. Hold the lock only for a snapshot: read the registry rows and journal state under the lock, release it, do the
   expensive per-slot work (census, git status, salvage checks) outside, then re-validate briefly under the lock only
   what the verdict needs (row generation and owner unchanged, no new journal). A slot that changed meanwhile is
   reported as changed-during-audit, never as a false finding.
3. Make it cheaper per slot or incremental: skip expensive checks for slots unchanged since the last audit (by
   generation, row mtime, and a cheap checkout fingerprint), and keep the census shared across slots (one /proc pass,
   not one per slot).
4. Say whether the Rust readers (#1122 parity; their soak is paused) would already help, and how much.

# Acceptance Criteria

- At 100+ slots, the hourly audit holds the registry lock for seconds, not minutes; total wall time drops
  measurably. Before and after numbers (wall, CPU, lock hold) are recorded in Notes.
- `--gate` verdicts are unchanged for the same state (a differential test against the current audit on a fixture
  with findings).
- A concurrent `adopt` or `create` is no longer starved: test or measure the wait behind a running audit.
- Rust parity filed or done if the Rust audit shares the shape.

# Notes

Reported by the lead coordinator from a consumer's profiling lane (2026-11-09). Same family as #1108 and #1142.

## Resolution

Delivered in four commits (turnstile, event pre-warm, doctor/status phases, pooled audit rows).

Measured first, at a consumer's scale (237 rows, 45,600 event files): audit already held the shared lock
only 6.9 s plus 0.08 s, because #797 had given it the snapshot shape. Its 370 s of wall time was
outside the lock: judging rows 187 s, cache planning 157 s, liveness 13 s. The long holders were
doctor and status, about 33 s each, because they held the lock for the whole run: event verification
7 s, storage probes 8 s, release replay 3 s, the liveness command 15 s. Overlapping readers also
starved writers: flock grants shared locks while a writer polls for an exclusive one.

- A waiting writer holds `ACTIVE.global.intent.lock`; readers pass through it first, so they queue
  behind a waiting writer. It is a flock on a descriptor, never the file's existence, and older
  releases ignore it safely. A reader defers at most 10 s.
- status, doctor, audit and unpushed verify the event chains before the lock; under it they only
  lstat each event file.
- status and doctor snapshot under the lock, measure outside it, and confirm under a second short
  lock that the registry's content identity is unchanged: every control file's digest, plus each
  machine's event count and last event digest. They re-measure only the changed rows, and after two
  repeats measure under the lock. Output and schema are unchanged.
- audit judges rows on `--jobs` threads (default min(4, CPUs)). Verdicts, reasons and order are the
  same as serial.

Before and after on a fixture with 120 slots, 40,000 events and a 60 ms liveness command, on the
shared gate host at load 85-110 (two runs each):

| command | wall (s) | CPU (s) | lock hold (s) |
|---|---|---|---|
| audit | 46.8, 42.8 -> 32.5, 30.6 | 38.0, 33.6 -> 37.2, 36.0 | 3.93, 2.30 -> 0.68, 0.61 |
| doctor | 13.6, 13.7 -> 13.9, 13.8 | 6.4, 6.4 -> 6.7, 6.5 | 13.45, 13.55 -> 0.54, 0.51 |
| status | 14.2, 13.5 -> 14.1, 13.7 | 7.0, 6.3 -> 6.8, 6.5 | 14.16, 13.40 -> 0.56, 0.51 |

A `hold` (a real mutation, `--wait-lock 300`) started behind looping status, doctor and audit
waited 134.0 s before and 3.3 s after.

Not verified: the consumer-scale run after the change. The fixture's Git checkouts are tiny, so
pooled row judging gains less there (27.4 s -> 11.4 s) than on large checkouts, and the measurements
ran with the privileged process census stubbed out.

The other edition's readers take no registry lock at all, so no parity issue is needed for the lock
shape. Its audit accepts `--jobs` (validated, rows judged serially).

Deferred, not done: skipping unchanged slots between audits (a sound checkout fingerprint costs
about as much as the Git calls it would replace), and making cache planning cheaper. Planning's
per-subject budget is sequential, so running it in parallel would change cache status, though not
verdicts.
