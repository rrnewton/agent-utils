---
title: 'wrkviz-muse-backend-rust: the muse summary backend in the Rust edition'
status: open
priority: 1
issue_type: feature
assignee: cl-start
created_at: 2026-11-04T18:13:01.570188000+00:00
updated_at: 2026-11-04T18:13:01.570188000+00:00
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
