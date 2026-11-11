---
title: 'wrkslots-memo-cap-regression: the read-only event-retention cap makes mutation loads re-read the whole log under the exclusive lock'
status: open
priority: 1
issue_type: bug
labels:
- wrkslots
- state-lock
created_at: 2026-11-11T03:36:44+00:00
updated_at: 2026-11-11T03:36:44+00:00
---

# Description

[watermelon-full-preview] Regression from #1247 wrkslots-status-lock (2473d23b), measured by a consumer lane (2026-11-10, 10:24 PM ET) on a copy of a consumer-sized registry (79,044 event files, 193 active rows, load 78-98). The parsed-event retention cap (256 MiB of event files; no whole verified chain over 5,000 events) applies to every command, so on a registry larger than the cap every event-log load is a full cold re-read. Mutations load the log several times while holding the exclusive project lock:

| command | before #1247 | after #1247 |
|---|---|---|
| adopt | 19.4 s (79,051 event-file reads) | 94.8 s (711,443 reads) |
| create | 30.5 s (79,052 reads) | 163.9 s (1,185,678 reads) |

# Design

Scope the retention caps to read-only scans. Mutations keep the parse-once behaviour the memo exists for (one load parses each event file once; later loads in the same command reuse it), which is safe for them: their memory is separately bounded by the workload, and the 640 GiB defect was a reader hashing a worktrees image, fixed by the registry-file allowlist, not by this cap. Readers (status, doctor, audit, unpushed) apply the caps when they start, beside their `--scan-memory-limit`, so the reader memory bound still holds.

# Acceptance Criteria

- A test counts event-file parses across repeated loads in one command on a synthetic registry larger than the reader cap: mutations parse each file once (reads == files), readers stay within the retained-bytes cap.
- Before/after reads and seconds for repeated loads on a consumer-sized control copy are recorded in Notes.

# Notes

Measured by mu-statuslock on a control-only copy (79,102 event files), three `_load_events` calls in one memo scope: mutation mode 79,102 parses / 12.7 s; read-only-bound mode 237,306 parses / 43.3 s. The fix restores the mutation column; readers keep the bound.
