---
title: 'agentctl-redact: rewrite a literal string across one agent processed history and archives'
status: closed
priority: 2
issue_type: feature
labels:
- agentctl
- redact
created_at: 2026-11-20T10:00:00-05:00
updated_at: 2026-11-20T10:00:00-05:00
---

# Description

[watermelon-full-preview] A consumer team asked for a way to rewrite a
literal string (a leaked secret, a stale identifier) out of one agent's
retained history without hand-editing JSON: `agentctl redact NAME
--literal STRING [--include-archived] [--dry-run]`.

# Design

- Scope: ONE agent's `queue/processed/*.json`, `events/*.jsonl`, and —
  with `--include-archived` — its archived records' processed queues.
- The command takes the registry lock, writes a 0600 backup of each file
  it touches under the registry's own `backups/` directory, and appends
  an audit line (who, when, file list, match count — never the string).
- It never touches the live inbox or in-flight entries, and refuses if
  the agent is mid-delivery.
- The string is rewritten to `"[redacted]"` only inside JSON string
  values; JSON stays valid.
- `--dry-run` lists the files and counts without writing.
- Both editions (Rust and Python).

# Acceptance Criteria

- Only matches inside JSON string values are rewritten; counts are exact.
- Dry-run lists files and writes nothing.
- The audit line never contains the literal string.
- Tests cover matches inside string values only, counts, dry-run, and
  the audit line's hygiene.

# Notes

[watermelon-full-preview] Lane mu-panedead/1963, queued by coordb after
#1944 agentctl-revive-missing-pane-dead-proof, requested by a consumer
team at normal priority (spec: 2026-11-20 backlog).

## Follow-ups from the fd484066 re-review (not in this change)

- Retired agents: only the newest archive is cleaned, and the retired
  fallback catches any load error rather than only not-found. Clean every
  archive of the agent and fall back only on not-found.
- The record is loaded before the lock; an agent archived in between can
  report a false clean result.
- Rust does not tighten or check an existing REGISTRY/backups directory
  (Python chmods it to 0700).
- Nits: Rust ignores a failure to take the queue locks and Python does not
  take them; strip a trailing CR from --literal-file; make the number
  refusal identical in both editions; `-adopt-failed` archives are
  skipped; help should say archives cover events too, not only processed
  queues; the common-word list never fires under the 12-character minimum.
