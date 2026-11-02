---
title: 'agentctl-handover: atomic agentctl handover OLD NEW'
status: open
priority: 2
assignee: mu-restore@devbig030
issue_type: feature
labels:
- agentctl
depends_on:
  agent-utils-740: parent-child
created_at: 2026-11-02T16:13:53.517942881+00:00
updated_at: 2026-11-02T16:13:53.517942881+00:00
---

# Description

[opus 5.5] From the 2026-11-02 host outage (a filesystem went read-only, two hard reboots, every agent died; restore was done by hand on two hosts). Owner goal: ONE idempotent command per team, safe to run any number of times against a fresh or existing herdr, with or without restored tabs, driven by a declarative spec of what should be running, run automatically after boot (except the certificate step).  A half-failed handover chain left the designation on a record that never started, stranding inter-team delivery ('start' picked a busy pane by label; 'coordinator set' still ran). Provide 'agentctl handover OLD NEW': start (fork-session if resuming), verify alive, responsive and bridged, designate, repoint bridges, then stop the old one. 'coordinator set' refuses a dead target; 'start' never reuses a pane it did not create. Pre-trust team-owned folders so a harness trust prompt cannot block an unattended start.
