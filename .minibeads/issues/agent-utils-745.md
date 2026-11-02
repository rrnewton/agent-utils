---
title: 'bridge-gap-backfill: chat bridges resolve an outage stream gap themselves and backfill'
status: open
priority: 1
issue_type: feature
assignee: mu-restore2@devbig030
labels:
- agentctl
depends_on:
  agent-utils-740: parent-child
created_at: 2026-11-02T16:13:53.510234959+00:00
updated_at: 2026-11-02T19:14:13.973845032+00:00
---

# Description

[opus 5.5] From the 2026-11-02 host outage (a filesystem went read-only, two hard reboots, every agent died; restore was done by hand on two hosts). Owner goal: ONE idempotent command per team, safe to run any number of times against a fresh or existing herdr, with or without restored tabs, driven by a declarative spec of what should be running, run automatically after boot (except the certificate step).  After the outage the chat bridge refused to start: 'provider stream has an unresolved gap ... automatic reconnect is refused'; retry-checkpoint-gap needs hand-computed evidence hashes, so the state was moved aside and re-initialised, losing retained requests and the outage's channel history (initial backfill 600 s). Resolve a gap automatically when the bridge has been down (with evidence recorded), backfill the outage window, and deliver the owner's unanswered messages. Also fix the misleading 'credential file missing: PATH' start message.
