---
title: 'tmp-stale-sweep: a periodic stale sweep for the six agent-utils temp patterns'
status: open
priority: 2
issue_type: task
labels:
- tmp
- cleanup
assignee: mu-tmpleak
created_at: 2026-11-23T14:45:00-05:00
updated_at: 2026-11-23T14:45:00-05:00
---

# Description

The six agent-utils temp patterns in /tmp (`agent-utils-gitconfig-global-*`, `agent-teams-relay-*`, `vibe-talk-wit-token-*`, `vibe-talk-chrome-unpack-*`, `vibe-talk-firstrun-*`, `wrkslotsd-validation-facts-driver-*.py`) are now removed at source on every exit path their code can run. A worker killed with SIGKILL, including CPU-timeout kills under gate load, cannot run that cleanup, so stale entries still accumulate slowly. No periodic sweep covers these patterns today: the host tidy timer only removes large old output files, and the generic tmpfiles aging has no pattern, open-handle, or content checks.

# Design

Run a periodic stale sweep for only these six patterns. An entry is deletable only when it is older than one hour, owned by the sweeping user, not open by any process (checked with a /proc fd scan or equivalent), and removed by its literal path in paced batches. Gitconfig sentinel directories are deleted only when the sentinel still holds exactly the seed bytes; a sentinel holding anything else is caught-write evidence and is kept and listed. Relay, wit-token, and driver entries carry an embedded PID and are deleted only when that PID is dead. Browser profile directories are deleted only when no live process references the path. Kept entries are listed, never silently dropped.

# Acceptance Criteria

- A periodic sweep (timer or equivalent) runs the six-pattern cleanup with the rules above and reports per-pattern before/after counts.
- A live entry (open handle, live embedded PID, or a gitconfig sentinel holding a caught write) is never deleted and is listed as kept.
- The sweep touches no entry outside the six patterns.
- Two consecutive sweeps an hour apart leave no deletable entry behind.
