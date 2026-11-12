---
title: 'discuss-unknown-terms-log: log Discuss unknown terms server-side in debug mode'
status: open
priority: 0
issue_type: feature
assignee: mu-discussgloss
labels:
- vibe-talk
created_at: 2026-11-12T19:31:00+00:00
updated_at: 2026-11-12T19:31:00+00:00
---

# Description

[watermelon-full-preview] Owner (Slack 013): "we should probably LOG all the 'terms I didn't understand' sections server side in a debugging mode for now ... and we should get more ideas about what appears in there." #1351 discuss-unknown-terms-metric already counts the terms on the log line; this issue persists them per run so frequencies over days can drive the #1394 discuss-glossary-preamble glossary. Cache-only, zero provider calls.

## Summary

Behind `[summaries] catchup_unknown_terms_log` (default off), append one JSONL record per Discuss run to `<state>/unknown-terms/unknown-terms.jsonl`, and add `vibe-talk unknown-terms-report` listing the most frequent terms over N days.

# Acceptance Criteria

- With the switch off, no file is written and Discuss behaviour is unchanged; with it on, each answered run appends one record with UTC time, scope, model, unknown_terms, count, and warm-up fingerprint.
- The log lives in its own subdirectory of the state directory, rotates to one backup at a size cap, and a failed append warns and never fails the answer.
- `unknown-terms-report [--days N] [--json] PATH` lists terms by count with first/last seen, reading the live file and its backup; no wall-clock-sensitive tests.
- Focused unit tests cover append, rotation, aggregation, and the report; the gate is green.

# Design

- New `vibe-talk/src/unknown_terms_log.rs`: record type, `log_path`/`state_dir_of`, size-capped `append_record` (1 MiB live plus one `.1` backup), clock-free `aggregate`/`cutoff_for_days`, and the report formatting/parsing.
- Config: `catchup_unknown_terms_log: bool` on `SummariesConfig` (false in the summary cache key, pinned by the destructure guard in `summarize/mod.rs`).
- Endpoint: after the existing tracing line in `http/api.rs::catchup`, best-effort append when enabled and a storage path exists.

# Notes

[watermelon-full-preview] #1393 discuss-unknown-terms-log, mu-discussgloss.

## Resolution

Pending landing.
