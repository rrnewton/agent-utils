---
title: 'mq-hooks-log-interleave: concurrent hook workers interleave inside hooks.jsonl lines'
status: closed
priority: 1
issue_type: bug
created_at: 2026-11-12T12:00:00.000000000+00:00
updated_at: 2026-11-12T14:00:00.000000000+00:00
closed_at: 2026-11-12T14:00:00.000000000+00:00
---

# Description

`log_line` in rs/merge-queue/src/hooks.rs appended each finished hook's
JSON line with `writeln!` on an unbuffered `File`, which formats through
several `write(2)` calls. Concurrent hook workers therefore interleave
inside a line: in one live `hooks.jsonl`, 282 of 630 lines did not parse
as JSON.

# Design

Format the whole line into one buffer and `write_all` it once, so each
appended line reaches the file in a single write under `O_APPEND`.
No lock file and no schema change.

# Acceptance Criteria

- N threads x M lines appended concurrently: every line in
  `hooks.jsonl` parses as JSON, exactly N*M lines, no lost or merged
  records (`concurrent_log_lines_never_interleave`, failing on the old
  code with interleaved fragments).
- Existing hook logging behaviour unchanged.

# Notes

## Resolution

[watermelon-full-preview] `log_line` now formats the whole line into
one buffer and `write_all`s it once under `O_APPEND`, so concurrent
hook workers cannot interleave inside a line. Regression test
`concurrent_log_lines_never_interleave` (8 threads x 50 lines, every
line parses, none lost) fails on the old code with interleaved
fragments and passes after the fix. Landing follows the usual gate.
