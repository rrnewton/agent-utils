---
title: 'herdr-run-e2e-readiness-race: the herdr-run differential''s readiness cases diverge under normal load'
status: open
priority: 1
issue_type: bug
assignee: mu-validate@devbig030
labels:
- herdr-run
- validation
created_at: 2026-11-03T11:40:29.473065688+00:00
updated_at: 2026-11-03T11:40:29.473065688+00:00
---

# Description

[opus 5.5] cross.herdr-run.differential fails intermittently in its end-to-end readiness cases under normal host load, with no other gate on the host: e2e/readiness-half-typed-line and e2e/reply-id-not-a-string (and, earlier, e2e/readiness-busy). In the handed-to-herdr call sequence the Python edition records a pane read (ignored=[3,32,33]) where the Rust edition records a pane run (ignored=[]). Seen in run 3 of the serial solo-phase stability runs (2 of 1868 cases) and once under the old one-step-at-a-time solo phase, so it predates the admission change: a readiness-probe race inside the step's e2e fixture or in one edition's readiness logic.
Raised to P1 on 2026-11-03: the step failed in about half of its recent solo runs (serial stability runs PASS, PASS, FAIL, FAIL; the solo-phase landing gate failed with 4 more divergences: e2e readiness-no-pane-text, failure-tab-list, raw-environment, cache-array).

Interim rule (devbig014 coordinator, 2026-11-03, replaces the earlier one-rerun rule): until this is fixed, a cross.herdr-run.differential failure that is ONLY in this family (e2e handed-to-herdr call-sequence divergences where one edition records a pane read and the other a pane run, e.g. readiness-half-typed-line, reply-id-not-a-string) may be rerun solo up to TWICE; any divergence outside the family blocks as usual.

# Design

Root-cause the race in cross/herdr_differential.py and the two editions' readiness paths: find which side decides to read versus run the pane and what timing it depends on (wall-clock probe windows, the fake Herdr's response timing). Fix the fixture so both editions see the same deterministic readiness sequence, or fix the edition whose decision depends on timing; follow the timeout rule (CPU budgets, wall clock about 8x) rather than lengthening sleeps.

# Acceptance Criteria

- The root cause is written in the issue before the fix.
- The affected cases pass 50 consecutive runs of the step under normal host load.
- No case is skipped or given a longer sleep as the fix.

# Notes

## Root cause

[muse watermelon-full-preview] Two load-dependent mechanisms, the first dominant. In every captured divergence the two editions' Herdr argument sequences are byte-identical; what differs is the ignored-signal list in the handed-to-herdr comparison text (`cross/herdr_differential.py` builds it in `_handed_line` as `index argv ignored=[...] environ=[...]`), flipping between `[]` and `[3, 32, 33]` on the same call in the same case.

The fake's `/bin/sh` launcher reads its own `/proc/$$/status` SigIgn at exec, so the recorded mask is the disposition set the *edition process was started with*, plus SIGQUIT, which the launcher's bash acquires by starting in script mode (measured directly: the launcher records `0x180000004` under edition-identical spawns). Both editions deliberately preserve their start mask and re-apply it to every Herdr child — `rs/herdr-run/src/client.rs` (`record_inherited_ignored_signals`, `bounded_output`'s `ignore_signals`, with the comment at lines 100-101 naming this exact hazard) and `py/herdr_run/client.py` (`bounded_control_command`: "Signal dispositions are handed on as the interpreter holds them. It starts no thread, so the C library never takes over a signal for its own use").

The start mask is therefore decided by the harness process's signal state at the moment it forks each step's edition process, and that state is not stable across a run: the C library takes signals 32 and 33 for itself once a process starts a thread (the same hazard the codebase already documents at `rs/herdr-run/src/client.rs:100-101` and `cross/herdr_differential.py:591`). Measured on this host: a Python parent hands children `[32, 33]` before it starts any thread and `[32]` after. Under load, thread starts and step spawns interleave differently, so the same logical call records different masks at random — which is why the step passes solo on a quiet host, fails in about half of loaded runs, and why the divergence count grows with contention (2, then 4, then 9 in the solo-phase stability runs and landing gate). No edition decides anything differently; this is ambient spawn-chain state being compared as if it were protocol behaviour.

Second mechanism, in the readiness cases only: `--wait-ready 0.2` against a 0.25 s poll interval that requires two consecutive ready readings, where each readiness assessment spawns real fake-Herdr processes and typed lines are rendered by a real bash. The number of assessment rounds that fit before the deadline varies with process-spawn latency, so one edition records an extra pane read where the other has already issued pane run. This is the `readiness-busy` / `readiness-half-typed-line` / `readiness-no-pane-text` shape.

## Fix direction

Make the handed environment a controlled fixture input instead of ambient state: start every e2e edition process with an identical explicit signal-disposition state from the harness (a preexec that resets dispositions to a canonical set), so the recorded mask is the same for every call in both editions whatever the harness's thread/exec history. The editions already preserve their start mask exactly, so no deliberate coverage is lost — the deliberate signals-ignored cases live in the unit-level invoke modes and in the rules-based `handed_ignored_signals` checker (`cross/herdr_differential.py:4666`), not in e2e steps, and no e2e behaviour drives signals. Then re-measure the readiness cases; if poll counts still flip there, make the fake's readiness sequence call-count deterministic rather than wall-clock timed. No longer sleeps anywhere.
