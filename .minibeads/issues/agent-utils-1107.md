---
title: 'audit-format-label-mismatch: the human audit labels BLOCKED what the JSON format calls UNKNOWN'
status: open
priority: 3
issue_type: bug
created_at: 2026-11-09T02:10:00.000000000+00:00
updated_at: 2026-11-09T02:10:00.000000000+00:00
---

# Description

The same `wrkslots audit` evidence gets different verdict labels depending on the output format. When only cache-census reasons remain on a row, `--format json` reports the slot UNKNOWN, while the human format prints, for example:

> BLOCKED: <slot>: cache inspection incomplete: bounded census is partial

(Seen on a consumer host, Python edition at agent-utils a0eeb67d, 2026-11-08.) An operator reading the human format sees a definite block where the machine format says "we don't know".

# Design

One verdict per row, computed once and rendered by both formats. The human output prints the same label as the JSON `verdict` (or status) field. Check both editions.

# Acceptance Criteria

- A test renders the same audit evidence in both formats and asserts identical per-row labels, including the cache-only-reasons case.
- Both editions agree, or the parity gap is recorded.

# Notes

Reported by the lead (devbig014) on 2026-11-08. Related: #1106 audit-cache-allowance-load.
