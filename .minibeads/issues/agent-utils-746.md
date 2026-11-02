---
title: 'restore-im-back: deterministic I''m-back summary and catch-up prompts after a restore'
status: open
priority: 1
assignee: mu-restore@devbig030
issue_type: feature
labels:
- agentctl
depends_on:
  agent-utils-740: parent-child
created_at: 2026-11-02T16:13:53.514054293+00:00
updated_at: 2026-11-02T16:13:53.514054293+00:00
---

# Description

[opus 5.5] From the 2026-11-02 host outage (a filesystem went read-only, two hard reboots, every agent died; restore was done by hand on two hosts). Owner goal: ONE idempotent command per team, safe to run any number of times against a fresh or existing herdr, with or without restored tabs, driven by a declarative spec of what should be running, run automatically after boot (except the certificate step).  After restoring, post a fixed summary to each coordinator's chat bridge (who was restored, from which session, downtime, who could not be restored and why), then send each restored agent a standard catch-up prompt, managers before lanes. Restore and coordinator messages must not block (a send to a revived lane blocked over 5 min mid-turn): deliver while working or return with the message queued.
