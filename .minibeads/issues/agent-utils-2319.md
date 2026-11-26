---
title: 'vibe-talk-discuss-browser-memory-hint: discuss-browser and summary-link-browser declare a Chromium-sized memory baseline so their learned cap stops OOM-killing them'
status: open
priority: 1
issue_type: task
assignee: cl-flaketriage
labels:
- vibe-talk
- validation
- flake
created_at: 2026-11-26T18:30:00-05:00
updated_at: 2026-11-26T18:30:00-05:00
---

# Description

`vibe-talk.check.discuss-browser` was red on 40 distinct gate heads in 7 days. In the retained gate output 18 of its 24 bad results are `NO_RESULT: OOM-KILLED (hit inner MemoryMax) at a learned memory cap` of about 1.25 GiB, and the dagrun profile stores agree: of 91 runs, 6 hit `oom_kill`, every one at the 1.25 GiB cap, while runs under a larger learned cap peaked at 1.42 GiB (median 1.18 GiB). The step declares `rss_baseline_bytes: 1073741824` (1 GiB), which is less than one real Chromium walk needs. `vibe-talk.check.summary-link-browser` has the same 1 GiB baseline and the same shape (48 runs, 1 `oom_kill`, peaks up to 1.30 GiB). A memory kill at a cap below the step's own working set is not a test result (owner rule 346).

# Design

Test-only DAG hint change: raise `rss_baseline_bytes` to 1.75 GiB (1879048192) in `vibe-talk/tests/dag.d/062-discuss-browser.yaml` and `vibe-talk/tests/dag.d/177-summary-link-browser.yaml`, so the cap derived from it (about 2.19 GiB) sits above the measured peaks with headroom. Nothing else changes: not the checks, not their timeouts, not other steps. `discuss-dock-browser` and `discuss-strike-live-browser` are NOT covered: their peaks are 0.40-0.60 GiB with no `oom_kill`, so their reds are not memory.

# Acceptance Criteria

- Both steps declare a 1.75 GiB `rss_baseline_bytes`, with a comment naming the measurement.
- The validation graph still loads and its contract tests pass.
- After landing, no `discuss-browser` or `summary-link-browser` NO_RESULT at a learned memory cap in the next gates that run them (checked from the gate ledger and step logs).

# Notes

Measurements by `cl-shardcap` from the dagrun profile stores (fields `peak_bytes`, `memory_max_bytes`, `memory_events_oom_kill`, `ok`, deduped by run id), relayed 2026-11-26. The other 6 `discuss-browser` failures in the retained logs are real assertions, not memory (2x `close/reopen prepared a different key`, 3x a `wait_for_function` timeout) and stay open for a main repro under this lane's triage. Related: `#2306` (agentctl shards' learned-cap OOM), `#2317` (NO_RESULT alarm).
