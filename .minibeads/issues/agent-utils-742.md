---
title: 'restore-team-spec: desired-state spec and an idempotent team-restore command'
status: open
priority: 1
assignee: mu-restore@devbig030
issue_type: feature
labels:
- agentctl
depends_on:
  agent-utils-740: parent-child
created_at: 2026-11-02T16:13:53.497507497+00:00
updated_at: 2026-11-02T16:13:53.497507497+00:00
---

# Description

[opus 5.5] From the 2026-11-02 host outage (a filesystem went read-only, two hard reboots, every agent died; restore was done by hand on two hosts). Owner goal: ONE idempotent command per team, safe to run any number of times against a fresh or existing herdr, with or without restored tabs, driven by a declarative spec of what should be running, run automatically after boot (except the certificate step).  A spec (restore.json or the team spec) lists who should be running: agents with tabs, launch commands, sessions, slots; bridges; units; herdr version. team-restore repairs whatever differs and leaves everything else retired (today restore revives every dead record, including finished one-off lanes). Resolve wrkslots and herdr from the team folder or profiles.json, never PATH alone (a non-login shell hung 300 s silently); start herdr if not running and print progress. The team's own pinned bin/agentctl is the only entry point; pin versioned binaries in the team folder and check versions (an in-place update left a herdr-0.8.0 file running 0.9.5). Check the user certificate first and print exactly what to run.
