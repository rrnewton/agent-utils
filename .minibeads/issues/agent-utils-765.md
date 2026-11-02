---
title: 'harness-identity-launcher-inode: stamp the harness identity after the launcher execs the real binary'
status: open
priority: 3
issue_type: task
labels:
- agentctl
created_at: 2026-11-02T16:26:35.771048866+00:00
updated_at: 2026-11-02T16:26:35.771048866+00:00
---

# Description

[opus 5.5] Queued by the coordinator (2026-10-28 to 2026-11-02). agentctl run-headless stamps the harness identity while the pid still runs the /usr/local/bin/claude bash launcher (inode = /bin/bash), before it execs the real claude binary, so executable_inode never matches and check --strict says the live coordinator is not live (014 lieutenant 2026-11-01; corrected: not the '(deleted)' exe theory). Stamp after exec (or re-stamp on first liveness check), or match by pid+starttime+boot.
