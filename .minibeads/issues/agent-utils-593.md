---
title: 'registry-agents-subdir: keep per-agent registry entries under agents/ and locks/'
status: closed
priority: 2
issue_type: feature
labels:
- agentctl
- layout
created_at: 2026-10-27T20:39:51.675211637+00:00
updated_at: 2026-10-27T23:10:02.968372879+00:00
closed_at: 2026-10-27T23:10:02.968372759+00:00
---

# Description

[opus 5.5] The agentctl registry mixes O(agents) entries (one directory per agent plus two lock files per agent) with the fixed items (coordinator.json, profiles.json, archive/, .progress/, ...). On a long-lived registry the fixed items are lost among hundreds of entries. Move per-agent directories to REGISTRY/agents/<name>/ and per-agent lifecycle locks to REGISTRY/locks/, in both editions, with a migration policy: new writes use the new layout, reads accept both for a 30-day deprecation period, an explicit 'agentctl migrate-layout' (dry run by default) and an automatic migration when a registry is opened for writing. Old binaries must fail safe on a migrated registry. Also add the repository-wide design rule: never mix an O(N)-growing collection with fixed items in one directory; audit the other tools and file issues for violations.
