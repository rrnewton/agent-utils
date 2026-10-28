---
title: 'herdr-tab-pinning: keep bridged agents first in the Herdr tab list (socket tab.move)'
status: open
priority: 2
issue_type: feature
labels:
- agentctl
created_at: 2026-10-28T17:39:22.229160349+00:00
updated_at: 2026-10-28T17:39:22.229160349+00:00
---

# Description

[opus 5.5] owner 13:12 ET: keep all bridged agents (lead coordinator first) at the front of the Herdr tab list. herdr 0.8.0 CLI has no tab move, but its socket API does: method "tab.move" {tab_id, insert_index} (protocol 19; also workspace.move). agentctl should call it on start/adopt and periodically (pin order: lead coordinator, other bridged agents, then lanes by age), configurable per registry.
