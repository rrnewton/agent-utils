---
title: 'merge-queue-donor-identity-and-prune: compare embedded revisions, not binary hashes; prune dead donor jobs'
status: closed
priority: 1
issue_type: bug
assignee: cl-mq@devbig030
labels:
- merge-queue
created_at: 2026-11-03T09:50:15.566290909+00:00
updated_at: 2026-11-03T13:02:55.610057728+00:00
closed_at: 2026-11-03T13:02:55.610057577+00:00
---

# Description

[opus 5.5] Follow-up to #807 merge-queue-donor-validators from the first live donor test on a consumer team's hosts.
1. Builds of the same merge-queue revision (0ce36eb6) are not byte-reproducible across hosts (the two hosts' binaries hash differently), so a tool identity based on a binary's file hash can never match between a queue host and a donor.
2. serve keeps donor jobs whose candidate has already landed or been superseded, so donors spend validation on dead candidates.
The stopgap for (1) is copying one host's exact binary to the other.

# Design

- Stamp the git revision and a dirty flag into the merge-queue binary at build time (build.rs), reusing the mechanism agentctl already uses to print "commit <sha>". `merge-queue --version` and the donor's identity probe print it.
- tool_identity compares revisions (plus the dirty flag), never file hashes. A binary with no embedded revision is reported with a warning and treated as a mismatch.
- serve prunes donor jobs whose candidate has landed or been superseded: it cancels the lease and drops the job, and the donor stops on its next renewal.

# Acceptance Criteria

- Two builds of the same revision on different hosts produce matching identities; a different revision or a dirty build does not.
- A binary without an embedded revision warns and is refused as a donor tool.
- A job whose candidate lands (or is superseded) while a donor holds its lease is cancelled; the donor's next renewal stops it, and no receipt for it is accepted.
