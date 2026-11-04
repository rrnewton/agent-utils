---
title: 'wrkviz-muse-backend: summarize with Muse Spark 1.3 by default, Anthropic and Codex still selectable'
status: closed
priority: 1
issue_type: feature
assignee: cl-start
created_at: 2026-11-04T15:18:18.837509464+00:00
updated_at: 2026-11-04T18:12:37.203372000+00:00
closed_at: 2026-11-04T18:12:37.203372000+00:00
---

# Description

The owner (2026-11-04 04:09 ET): "for the wrkviz batch jobs, it seems like we could switch to spark immediately. I'd like to do that for future summarization."

`wrkviz summarize` today calls Anthropic (or Codex) models, and the cost estimates for summarizing a large log archive run to thousands of dollars. Muse Spark 1.3 (`muse-spark-1.3-internal`, reached through the headless `muse` CLI) is zero-rated for this use.

# Design

- A `muse` summarize backend in wrkviz that runs the headless muse CLI with model `muse-spark-1.3-internal`. It becomes the default backend; Anthropic and Codex stay selectable in the config, as they are today.
- The responses must fill the existing summary cache entries (same keys and shape), so cached work stays valid and a mixed cache works.
- `--plan` reports zero model spend for the muse backend, plus its expected wall time.
- Concurrency and timeouts suit a local CLI (no API rate-limit assumptions); a failed or malformed answer is retried, then stops the run, as with the other backends.

- Folded in from bakeoff round 2 (harness `ai_docs/2026-11-03-summarization-bakeoff.md#round-2-wrkviz-pipeline`, wrkviz's own pipeline with only the backend swapped):
  - (a) a native Muse backend, replacing the bakeoff's ~60-line shim behind `--claude-command`;
  - (b) clearer phase time bounds in the phase prompt: wrkviz drops bullets timestamped outside their phase, and one model lost 2 phases to the fallback heuristic;
  - (c) serialize or time-limit Codex backend calls: two Codex models stalled ~20 min when run in parallel;
  - per-job-kind model choice: Spark 1.3 for phases, Watermelon Full for hourly rollups (the suggested split); validate on a second sample.

# Acceptance Criteria

- One real team-day summarized end to end with the muse backend against a scratch copy of an existing archive's cache. Every new entry has the existing cache shape, and the rendered site loads it.
- Default backend is muse; `--backend claude|codex` (or the config equivalent) still works; unit tests cover backend selection and cache-shape compatibility.
- CLI help and the userguide describe the backend, its model and its defaults.
- Gate passes; no client names in the tree.

# Notes

Priority P1 by owner direction. Related: the vibe-talk summarizer on a Model API key waits for the owner's key (separate work).

## Resolution

[opus 5.5] cl-start, 2026-11-04. The Python edition (the primary one) has the backend; the opt-in Rust edition has the same CLI surface and refuses muse until #842 wrkviz-muse-backend-rust.

- `--backend` defaults to `muse`, a native backend (`wrkviz/muse_backend.py`): `muse exec --json --prompt-file --output-schema --preset empty`, with shell, writes, web, reminders, questions, the session log, personal context and Muse rules off. Usage comes from `run_usage` and the answer from `run_terminal`. It reuses the Claude batch path, so its entries fill the same cache shape; a unit test compares a muse entry and a Claude entry field by field.
- Models: `muse-spark-1.3-internal` for phases, names and the overview; `watermelon-full-preview` for calendar rollups. `--model` and `--rollup-model` override them; rollups are cached and calibrated under the rollup model.
- **Spark finding:** with the open schema, Spark answered 6-job batches with a single entry, with invented keys, or with an extra "Duplicate placeholder to force failure" entry, and even a one-job batch came back with a duplicate entry. 5 of the first 18 calls failed and stopped the run. Muse batches now get a per-batch schema (the `key` is an enum of the batch's keys, and the array has exactly that many items). Since then, 0 of 17 calls have failed, at the default batch size of 6. Documented in the userguide.
- (b) The phase prompt states each key's half-open window in UTC and in ms. No bullet was dropped in any of the 87 muse phases.
- (c) `--backend-timeout SECONDS` (default 900, both editions) stops a stalled call's process group; the batch fails like any provider error, and finished batches stay cached.
- `--plan`: muse is zero-rated (`$0.00 model spend`, and a budget never stops it), and its plan prints an expected wall time from the receipts' call durations or built-in per-model figures. Paid backends' plans are unchanged.
- Docs: CLI help, quickstart, userguide, README.
- Rust: help, usage and quickstart are byte-identical to Python's (the parity tests are unchanged). `--backend muse`, including the default, exits 2 with a one-line error pointing to #842, as does a `--rollup-model` that differs from `--model`. `--backend-timeout` is enforced.

Validation ran on a scratch reflink copy of a real archive, never the original. One team's 2026-10-26 ET (43 phases, 11 names, the overview, technical and plain daily rollups) finished in 217 s after the schema fix, and 2026-10-25 ET (44 phases, 2 names, rollups) in 671 s, both $0 and clean. The new entries have the same envelope, artifact and result fields as the team's existing Claude entries. `wrkviz build` published them: in headless Chromium the site loaded with no page errors and fetched the team's spine shard, which contains the muse phrases, and `./timeline show` renders a muse phase. Watermelon Full rollup calls took anywhere from 40 to 480 s.
