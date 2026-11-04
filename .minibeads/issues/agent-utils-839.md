---
title: 'wrkviz-muse-backend: summarize with Muse Spark 1.3 by default, Anthropic and Codex still selectable'
status: open
priority: 1
issue_type: feature
assignee: cl-start
created_at: 2026-11-04T15:18:18.837509464+00:00
updated_at: 2026-11-04T15:18:18.837509464+00:00
---

# Description

## Description

The owner (2026-11-04 04:09 ET): "for the wrkviz batch jobs, it seems like we could switch to spark immediately. I'd like to do that for future summarization."

`wrkviz summarize` today calls Anthropic (or Codex) models, and the cost estimates for summarizing a large log archive run to thousands of dollars. Muse Spark 1.3 (`muse-spark-1.3-internal`, reached through the headless `muse` CLI) is zero-rated for this use.

## Design

- A `muse` summarize backend in wrkviz that runs the headless muse CLI with model `muse-spark-1.3-internal`. It becomes the default backend; Anthropic and Codex stay selectable in the config, as they are today.
- The responses must fill the existing summary cache entries (same keys and shape), so cached work stays valid and a mixed cache works.
- `--plan` reports zero model spend for the muse backend, plus its expected wall time.
- Concurrency and timeouts suit a local CLI (no API rate-limit assumptions); a failed or malformed answer is retried, then stops the run, as with the other backends.

## Acceptance Criteria

- One real team-day summarized end to end with the muse backend against a scratch copy of an existing archive's cache. Every new entry has the existing cache shape, and the rendered site loads it.
- Default backend is muse; `--backend claude|codex` (or the config equivalent) still works; unit tests cover backend selection and cache-shape compatibility.
- CLI help and the userguide describe the backend, its model and its defaults.
- Gate passes; no client names in the tree.

## Notes

Priority P1 by owner direction. Related: the vibe-talk summarizer on a Model API key waits for the owner's key (separate work).
