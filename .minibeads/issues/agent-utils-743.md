---
title: 'restore-installed-units: persistent user units instead of transients, installed from the spec'
status: open
priority: 1
assignee: mu-restore@devbig030
issue_type: feature
labels:
- agentctl
depends_on:
  agent-utils-740: parent-child
created_at: 2026-11-02T16:13:53.501647376+00:00
updated_at: 2026-11-02T16:13:53.501647376+00:00
---

# Description

[opus 5.5] From the 2026-11-02 host outage (a filesystem went read-only, two hard reboots, every agent died; restore was done by hand on two hosts). Owner goal: ONE idempotent command per team, safe to run any number of times against a fresh or existing herdr, with or without restored tabs, driven by a declarative spec of what should be running, run automatically after boot (except the certificate step).  Every chat bridge, watcher and timer was a systemd-run transient and vanished on reboot; they were recreated by hand from a services ROLLBACK.sh. install-units writes persistent user units (bridges, watchers, herdr server, boot-time team-restore waiting for the certificate) from the team spec; services refresh updates them in place; team-restore starts any that are missing.
