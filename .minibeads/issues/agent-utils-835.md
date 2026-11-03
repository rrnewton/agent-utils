---
title: 'change-classifier-known-nodes: refuse a classification whose payload names an unknown node'
status: closed
priority: 2
issue_type: feature
assignee: cl-mq@devbig030
labels:
- change-classifier
created_at: 2026-11-03T22:28:17.417117210+00:00
updated_at: 2026-11-03T22:37:57.360103824+00:00
closed_at: 2026-11-03T22:37:57.360103664+00:00
---

# Description

[opus 5.5] Requested by a consumer team that adopted change-classifier (#823) with conditions: their own selector stays authoritative, may call the crate, keeps its three checks as a post-pass, and refuses any final selection smaller than its own. They asked for the optional known-node check offered in the #823 differential report: their selector refuses when a matched rule payload names a node that is not in the current node list (a stale map), and the crate does not.

# Design

- `change-classify classify --known-nodes FILE` (and the library equivalent) takes a list of known node names; when a matched rule's payload names a node not in the list, the classification is refused with the offending node and rule named, never silently trimmed.
- Optional: without --known-nodes, behaviour is unchanged.
- Add golden vectors for the check to the shared vector file.

# Acceptance Criteria

- Tests and vectors: a payload naming an unknown node is refused with the node and rule named; all-known payloads classify as before; no flag means unchanged behaviour.
- --help documents the flag and the file format.
