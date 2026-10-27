---
title: 'gh-paced-state-subdirs: keep snapshot directories and corrupt-file asides out of the state root'
status: open
priority: 3
issue_type: task
labels:
- gh-paced
- layout
depends_on:
  agent-utils-593: related
created_at: 2026-10-27T22:15:30.711082676+00:00
updated_at: 2026-10-27T22:15:30.711082676+00:00
---

# Description

[opus 5.5] Found by the #593 registry-agents-subdir audit (AGENTS.md rule: a growing collection gets its own directory). The gh-paced state directory ($XDG_STATE_HOME/gh-paced) holds the per-account files (<acct>.json, leases, cooldowns) beside one snap-<pid>-<start>-<nonce>/ directory per running invocation (rs/gh-paced/src/snapshot.rs) and <acct>.json.corrupt-<n> asides that are never pruned. Snapshots are swept when abandoned, so they stay bounded by concurrency, but they still mix with the fixed files; the asides grow without bound. Low priority.

Proposed fix: snapshots/snap-.../ and corrupt/<acct>.json.<n>, with readers and the sweep accepting the old places for a deprecation period.
