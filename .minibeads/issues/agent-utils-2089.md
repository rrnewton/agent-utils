---
title: 'agentctl-restore-multi-project-images: restore must remount the slot images of every configured wrkslots project'
status: open
priority: 2
issue_type: bug
labels:
- agentctl
- wrkslots
created_at: 2026-11-22T05:40:00-05:00
updated_at: 2026-11-22T05:40:00-05:00
---

# Description

`agentctl restore` runs `wrkslots image status` (and remounts) for a single wrkslots project's image. A consumer is adding a second wrkslots project with its own btrfs image; after a reboot, a lane revived into that project would start in an empty mount point.

# Design

Let restore handle every configured wrkslots project: discover the projects (from a registry-level list or the agents' recorded slot projects) and run the image status and remount step for each before reviving agents, or accept a wrkslots wrapper (AGENTCTL_WRKSLOTS_BIN) that covers several projects; report each project's result in the restore report.

# Acceptance Criteria

- After a simulated reboot, agents in two different wrkslots projects revive into mounted slots; a project whose image fails to mount is reported and its agents are held, not started in an empty mount; deterministic tests.

# Notes

Filed by the coordinator from a consumer request; the owning lane refines this file as it works.
