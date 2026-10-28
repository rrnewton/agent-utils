---
title: 'herdr-tab-label-drift: one source of truth for agent names; tab labels follow the registry'
status: open
priority: 2
issue_type: feature
labels:
- agentctl
created_at: 2026-10-28T17:39:22.189978495+00:00
updated_at: 2026-10-28T17:39:22.189978495+00:00
---

# Description

[opus 5.5] owner 13:04 ET split-brain: two dev-hermit tabs labelled "coord3" (wJ:tB8 #360 is really chat-coord, a renamed old bridge agent; wJ:tDC #428 is coord3). agentctl should keep Herdr tab labels in sync with registry names (set on start/adopt, re-check periodically), warn on duplicate labels across registries, and watch-agents should flag drift. Owner 13:11 ET, broader: one source of truth for an agent's NAME across the suite: the agentctl registry name (unique per registry; host-qualified across registries, e.g. coord3@devbig030) determines the Herdr tab label, the sender header in agentctl send / agent-teams messages ([from NAME via ...]), and what a parent calls its subagent; move the best-effort who-am-i (hermit-specific today) into agent-utils as `agentctl whoami` that answers from the registry (pane -> record), falling back to heuristics only when unregistered; enforce uniqueness at start/adopt and refuse or flag duplicates.
