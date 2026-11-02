---
title: 'merge-queue-serve-sync-label: serve syncs a ready label inside its cycle'
status: open
priority: 2
assignee: cl-mq@devbig030
issue_type: feature
labels:
- merge-queue
created_at: 2026-11-02T23:56:36.134199027+00:00
updated_at: 2026-11-02T23:56:36.134199027+00:00
---

# Description

[opus 5.5] From a consumer team 2026-11-02 (the owner wants labelled PRs never missed): 'merge-queue serve' only watches submitted items and polls the base; it never syncs a label, so a PR labelled ready that nobody submits is never ingested. Proposal: 'serve --sync-label <label> --sync-every <secs>', running sync inside serve's existing cycle under the queue lock (no second writer), reusing sync's batched list call and the approval-at-head filter. Sync failures are logged per cycle and never block validation. Interim: the consumer runs a timer calling 'merge-queue sync --label <label>' and retires it once this ships.
