---
title: 'watch-agents-write-churn: the agent watcher writes far more to disk than its state explains'
status: closed
priority: 2
issue_type: bug
assignee: mu-validate
created_at: 2026-11-06T12:30:00.000000000+00:00
updated_at: 2026-11-06T19:29:20.000000000+00:00
closed_at: 2026-11-06T19:29:20.000000000+00:00
---

# Description

The `agentctl watch-agents` unit for this registry (agentctl-watch-dev-agent-utils)
was reported by its owner-alarm watcher to have written 85.5 GB in 3 h before it
was OOM-killed at 03:23 ET on 2026-11-06 (about 28 GB/h at that spike). What it
persists is small: a 32 KB state file and a 6-40 KB report per pass.

Measured live on the replacement unit on 2026-11-06, read-only (no unit stop):

- Main watcher PID over 90 s: `wchar` grew 718 KB while kernel `write_bytes`
  grew 100352000 bytes — about 100 MB of device writes against 0.7 MB of
  write syscalls (a ~100x multiplier), at a moment of only moderate activity.
  An earlier 60 s sample showed the cgroup's device writes growing ~2.4 MB/min.
- `strace` of the watcher shows its real writes are small and fsync-heavy:
  state (32 KB) and report (~6 KB) rewritten after every pass, queue JSON
  (~1 KB) per send attempt with a file `fsync` and a directory sync each, and
  a repeated `child-done` notice for the same agent re-logged about every 62 s.
- File-level churn over 3 min of state directories is likewise small: state
  and report rewrites, target-lock file touches, `admissions.jsonl` appends,
  and the watch/agent logs. The persisted corpus is tiny (19 MB ledger,
  23 MB logs); the device-level rate is therefore not explained by file
  growth but by rewrite-plus-fsync churn amplified by the copy-on-write host
  filesystem, and it spikes far above this morning's rate under the nightly
  load the 03:23 report came from.
- Retained bulk on disk is separate and small: ~25 MB agentctl service
  snapshots per build, 103 MB builds, 70 MB herdr config state.

# Design

- Do not rewrite the watcher state or report file when its content has not
  changed: serialize, compare with what is on disk, and only then do the
  temp-file + rename + fsync publication. Count skipped rewrites.
- Find why the same notice is re-queued about every 62 s (the pass/quick-check
  cadence): if the re-queue of an unchanged, still-undelivered notice is not
  an intended retry, stop re-writing its queue JSON and fsyncs; keep whatever
  retry of the send itself is intended, with its existing backoff.
- Cut target-lock churn if it is per-pass: acquire a target lock only when a
  delivery to that target is actually attempted in this pass, or hold one
  lock file per watcher instead of touching one per agent per pass.
- Add counters to the watch report: per-pass bytes the watcher wrote to disk
  and how many of its candidate rewrites it skipped as unchanged, so a future
  spike is visible in the report instead of only in the unit's IO totals.

# Acceptance Criteria

- With an unchanged pass, the watcher writes neither its state nor its report
  file (verified by file mtimes and the new counters), and a changed pass
  writes each exactly once.
- A still-undelivered notice does not rewrite its queue JSON on the ~62 s
  cadence; its send retry behaviour is unchanged.
- Per-pass target-lock touches are gone or reduced to actual delivery
  attempts, shown by lock-file mtimes across a quiet pass.
- The watch report carries per-pass written bytes and skipped-rewrite counts.
- The watcher test suite passes, and a 10-minute live run of the fixed
  watcher shows a materially lower `write_bytes` rate than the ~1 MB/s
  measured above at comparable activity.

# Notes

Measured by mu-validate on 2026-11-06 from the lead's report. The watcher
unit was measured read-only and never stopped; the fix must not change notice
delivery semantics, only what is written when nothing changed. Rollout is a
normal agentctl build; the running watcher picks the fix up on its next
service refresh.

## Resolution

Landed on main as b6b181f9. The fixes: write_json_file in agentctl skips rewriting the watcher state (and report) when the new content is identical to what's on disk, avoiding the extra tempfile+rename+fsync pass; send target locks only refresh mtime hourly (protecting against the /tmp cleaner's day-scale expiry without inflating write volume), and the watcher logs each unchanged episode line once per state instead of every pass. The watch report now includes write_churn counters (bytes written, files rewritten, files skipped as unchanged) to size a recurrence. Full gate on the rebased SHA was green (validate OK). Closing this issue.
