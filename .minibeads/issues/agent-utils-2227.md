---
title: 'agentctl-services-process-children-fallback: the services host reader finds child processes on kernels without /proc/PID/task/TID/children'
status: open
priority: 3
issue_type: bug
labels:
- agentctl
- services
created_at: 2026-11-24T18:35:00-05:00
updated_at: 2026-11-24T18:35:00-05:00
---

# Description

`rs/agentctl/src/services/host.rs` `process_children` reads `/proc/PID/task/PID/children`. That file exists only with `CONFIG_PROC_CHILDREN`, which some production kernels don't set. When it's missing, the function returns an empty list, so it fails open. As a result, when a unit's script starts agentctl without `exec` (a shell parent with agentctl as its child), `agentctl services refresh` (refresh.rs) doesn't find the agentctl child and waits out its health timeout instead of proving health.

The other process readers in the tree already handle the missing file. They fall back to scanning `/proc/*/stat` for a matching parent pid, or they say "unknowable": `client.rs`, `foreign/ownership.rs` (two sites), `client.py`, `ownership.py` (two sites). Reported by a consumer team's infrastructure lead on 2026-11-24; there's no incident so far.

# Design

- When the children file is absent, `process_children` falls back to a parent-pid scan of `/proc/*/stat` (field 4), matching the existing readers' fallback. It returns a typed result that distinguishes "no children" from "couldn't determine". The refresh then reports "unknowable" instead of silently waiting out the timeout.
- Share one helper for this across the Rust readers if that's practical, so the fallback lives in one place.

# Acceptance Criteria

- Test (fake /proc root): with no children file, a parent with one child returns that child through the stat scan.
- Test: an unreadable /proc gives "couldn't determine", and the refresh health check says so in its report, not a silent timeout.
- Test: with the children file present, behaviour is unchanged.

# Notes

Seen at an older pinned build; current main has the same read (host.rs process_children).
