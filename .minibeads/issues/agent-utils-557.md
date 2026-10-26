---
title: 'agent-teams-cancel-perms: agent-teams creates queue tombstone directories and files with umask modes'
status: open
priority: 1
issue_type: bug
labels:
- agent-teams
- agent-queue
created_at: 2026-10-26T21:32:19.832064604+00:00
updated_at: 2026-10-26T21:32:19.832064604+00:00
---

# Description

[opus 5.5] agent-teams' write_tombstone creates QUEUE/cancel with fs::create_dir_all (0755 under the usual umask) and writes the tombstone with a plain create (0644). agent-queue requires every queue state directory to be private, so on 2026-10-26 the chat bridge's lookups in the coordinator's queue failed with 'queue state directory is not private: .../queue/cancel' from 13:20 until a manual chmod at 13:51. The hook-inbox deliverer has the same pattern for its inbox, cancel and claimed directories. Part of #556 chat-delivery-latency.

# Acceptance Criteria

Queue tombstones go through agent-queue's own cancel (private lane, 0600 file, set aside at once when no drain holds the queue). Hook-inbox directories are created 0700 and items 0600 whatever the umask. A test runs with umask 022 and checks the modes.
