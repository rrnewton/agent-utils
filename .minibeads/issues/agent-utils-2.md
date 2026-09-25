---
title: 'gchat-deployment-handoff: reproduce the mobile reader on the intended host'
status: open
priority: 0
issue_type: task
labels:
- vibe-talk
- deployment
depends_on:
  agent-utils-1: parent-child
created_at: 2026-09-25T01:32:01.933921963+00:00
updated_at: 2026-09-25T01:33:45.758377897+00:00
---

# Description

[gpt-5.6-sol] Recreate the already-proven provider-neutral app plus private adapter deployment on the intended host. Keep private host, credential, and organization-specific setup out of this repository.

# Acceptance Criteria

Managed mobile Chrome opens the deployed URL; real chat rows render; push is connected; services survive logout and restart; no unintended deployment remains on the source host.

# Notes

A prototype was validated on a temporary source host and then deliberately stopped and disabled after discovering it was the wrong machine. Reproduce it on the intended host from the private handoff; do not copy host-specific configuration into this repository.
