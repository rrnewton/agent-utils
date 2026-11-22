---
title: 'validation-page-build-output-race: quote-dismiss-browser races build.page over the shared page output directory'
status: open
priority: 2
issue_type: bug
labels:
- testing
created_at: 2026-11-22T10:25:00-05:00
updated_at: 2026-11-22T10:25:00-05:00
---

# Description

vibe-talk.check.quote-dismiss-browser dies in 1-2 seconds with ENOENT reading target/web/voice.html in several gates: its negative-control build writes under target/web while the parallel build.page step, its declared dependency, replaces the same directory.

# Design

Give the negative-control build its own output directory (or serialize it after build.page with a declared resource) so no step rebuilds or replaces another step output; audit other browser checks for the same pattern.

# Acceptance Criteria

- quote-dismiss no longer touches the shared page output; a test or DAG check asserts no two concurrent steps write the same output directory.

# Notes

Filed by the coordinator; the owning lane refines this file.
