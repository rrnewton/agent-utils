---
title: 'backlog-to-tracker: move the coordinator''s private backlog into the local tracker'
status: closed
priority: 1
issue_type: task
labels:
- tasks
- coordinator
depends_on:
  agent-utils-631: parent-child
created_at: 2026-10-28T13:57:16.378068086+00:00
updated_at: 2026-10-28T14:20:48.521764979+00:00
closed_at: 2026-10-28T13:59:13.202319046+00:00
---

# Description

[opus 5.5] Part of #631 task-tracker-adapters (near-term fix 2). The coordinator kept dispatchable work in a private notes file instead of the tracker, so none of it showed in mb ready, had a priority, or could block or be blocked.

Wanted: every dispatchable entry of that file filed as an issue (generic wording; host-private operations and owner-pending decisions stay in the private file, which from now on holds only those), and the coordinator files new work in the tracker first.

# Notes

[opus 5.5] Done: the 11 dispatchable entries were filed as #637 agent-teams-coordinators-repo-url, #638 wrkslots-test-loop-leak, #639 agent-kind-misdetect, #640 wrkviz-invented-key-retry, #641 gate-flake-graph-wrkslots, #642 watcher-api-error-idle, #643 agentctl-test-process-leak, #644 chat-rescan-incremental, #645 upstream-search-deferred, #646 cache-shadow-thread-replies and #647 archive-stale-branch (each discovered-from this issue). Left in the private file on purpose: host-private operations (voice stack, host certificates, per-host configuration), decisions waiting on the owner (adoption plan, liaison agent, landed-services-only voice, which GitHub identity posts, a port-binding rebuild), and entries already done or filed (#557 agent-teams-cancel-perms, #569 vibe-talk-test-debuginfo, #581 bridge-git-timeout-fatal, #608 server-side-search, #613 fts-marker-residue). Remaining for the coordinator: prune the migrated lines from the private file, and file new work in the tracker first (#632 task-dispatch-protocol).
