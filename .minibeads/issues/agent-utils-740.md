---
title: 'team-restore: one idempotent restore command per team from a desired-state spec'
status: open
priority: 1
assignee: mu-restore@devbig030
issue_type: epic
labels:
- agentctl
created_at: 2026-11-02T16:13:53.489220659+00:00
updated_at: 2026-11-02T16:13:53.489220659+00:00
---

# Description

[opus 5.5] From the 2026-11-02 host outage (a filesystem went read-only, two hard reboots, every agent died; restore was done by hand on two hosts). Owner goal: ONE idempotent command per team, safe to run any number of times against a fresh or existing herdr, with or without restored tabs, driven by a declarative spec of what should be running, run automatically after boot (except the certificate step).  This epic tracks the gap list (25 gaps found during the restore) as issues #741-#747. Out of scope here: persistent host journal (host setup). The hung-TUI health check (working by title with a blank grid) is tracked separately.
