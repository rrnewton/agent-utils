---
title: 'config-change-granularity: removal fingerprints the parsed policy it depends on'
status: open
priority: 2
issue_type: bug
labels:
- wrkslots
created_at: 2026-11-15T14:36:00+00:00
updated_at: 2026-11-15T14:36:00+00:00
---

# Description

[watermelon-full-preview] Editing the project config aborts
in-flight removals twice over: the mutation-lock entry compares
the whole parsed config, and the lock-scope fingerprint compares
raw file bytes, so even a comment-only edit during a long
lock-free salvage discards the gather (per batch item). Coherence
is the right goal; whole-file granularity is the defect.

# Design

Fingerprint the parsed policy subset removal actually depends on
(cache globs, remotes, hooks, disk and salvage policy), and give
batch removal the existing bounded re-gather treatment instead of
a hard abort when only unrelated fields changed. A relevant change
still refuses and preserves everything.

# Acceptance Criteria

- Comment/format-only config edits during salvage do not abort.
- A cache-glob or remote change mid-removal still refuses.
- Batch items re-gather a bounded number of times, then report.
