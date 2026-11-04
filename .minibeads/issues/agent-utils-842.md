---
title: 'wrkviz-muse-backend-rust: the muse summary backend in the Rust edition'
status: closed
priority: 1
issue_type: feature
assignee: cl-start
created_at: 2026-11-04T18:13:01.570188000+00:00
updated_at: 2026-11-04T18:51:03.184661000+00:00
closed_at: 2026-11-04T18:51:03.184661000+00:00
---

# Description

#839 wrkviz-muse-backend gave the Python edition of wrkviz a zero-rated `muse` summary backend and made it the default. The opt-in Rust edition (`WRKVIZ_ENGINE=rust`) has the same CLI surface (help, usage and quickstart are byte-identical, as the parity tests require), but it refuses `--backend muse`, including the default, with "the muse backend is not yet in the Rust edition (#842 wrkviz-muse-backend-rust); use the Python engine or --backend claude|codex". It also refuses a `--rollup-model` that differs from `--model`. It already enforces `--backend-timeout`.

# Design

Port the Python edition's behaviour (py/wrkviz, #839) to rs/wrkviz:

- a muse runner beside the Claude one: `muse exec --json --prompt-file --output-schema --preset empty` plus the isolation flags; usage from `run_usage` events, the answer from `run_terminal`;
- per-batch narrowed schemas for muse summary and name batches (the key enum and exact item count; see the #839 notes for why);
- the phase prompt's time-window lines, byte-identical to Python's;
- per-kind models: `rollup_model` for calendar rollups in the plan, the run, calibration and the run request;
- the zero-rated plan: $0.00 model spend, no spend guard, `estimated_seconds` and the wall-time line for zero-rated backends only;
- remove the refusals once the above is in.

# Acceptance Criteria

- The Python-vs-Rust differential tests cover muse with a fake `muse` wrapper (heuristic-style tree and stdout comparison), plus `--rollup-model` and `--plan` under muse.
- The refusal tests from #839 are replaced by parity tests.
- The gate passes.

# Notes

Filed by cl-start on coordinator direction, 2026-11-04.

## Resolution

[opus 5.5] cl-start, 2026-11-04. The Rust edition now has the muse backend at parity with Python, and the #839 refusals are gone.

- `rs/wrkviz/src/muse_backend.rs` holds the runner: the same `muse exec` arguments, event parsing and error messages as `py/wrkviz/muse_backend.py`. It plugs into the Claude-shaped batch path of summaries and names, and `SummarizeOptions::agent_command` hands it `--muse-command`.
- Muse batches get the narrowed answer schema (`output_schema(Some(keys))`, the key enum plus an exact item count). The phase prompt carries the time-window lines.
- `rollup_model` is used for rollup lookup, carry-forward, generation, calibration, and the run request (recorded only when it differs from the model).
- The plan: `ZERO_RATED_BACKENDS`, per-kind call times from receipts or built-in figures, `estimated_seconds`, and the wall-time line, all for zero-rated backends only. A budget needs no `--price` for muse and starts no spend guard.
- CLI: muse is the default, with Spark for phases and Watermelon Full for rollups when `--model` is omitted, and `--muse-command` / `--rollup-model` are honoured.
- Tests: the refusal tests are replaced by differential tests with a fake `muse`. They cover the default run (identical prompt and schema digests for every call, plus the archive tree, stdout and stderr), `--plan` and a budget under muse, a failed run, a split `--rollup-model`, and a bad `--backend-timeout`.

Validation on the #839 scratch reflink copy, never the archive:
- The Rust plan sees every Python muse entry for 2026-10-25 and 2026-10-26 as cached, and its 10-26 plan matches Python's line for line.
- A Rust run of 2026-10-24 (54 phases, 16 names, 2 daily rollups) finished in 13 batches with no retries, in 524 s and at $0.
- Python then plans that day as fully cached.

