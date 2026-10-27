---
title: 'registry-old-layout-removal: drop registry layout 1 support after 2026-11-26'
status: open
priority: 2
issue_type: chore
labels:
- agentctl
- layout
- due-2026-11-26
depends_on:
  agent-utils-593: related
created_at: 2026-10-27T22:05:53.012035384+00:00
updated_at: 2026-10-27T22:05:53.012035384+00:00
---

# Description

[opus 5.5] DUE 2026-11-26 (end of the 30-day deprecation period of #593 registry-agents-subdir). Until then both agentctl editions, agent-teams, the wrkslots liveness probe and wrkviz read registry layout 1 (REGISTRY/<name>/ with REGISTRY/.<name>.lock) beside layout 2 (REGISTRY/agents/<name>/, REGISTRY/locks/), and commands that write migrate idle layout-1 agents automatically.

After the date, once every live registry reports layout 2 (agentctl migrate-layout prints layout: "2"):
- rs/agent-queue/src/registry.rs: locate() and lock_paths() answer layout 2 only; drop legacy_names(), legacy_lock_files() and the Legacy location; keep the tripwire (older builds may still exist on other hosts).
- rs/agentctl/src/registry_layout.rs and py/agentctl/registry_layout.py: keep migrate-layout as a one-shot tool or make it refuse with a pointer to an older release; remove auto_migrate and its CLI hooks (cli.rs writes_registry, py cli _auto_migrate).
- py/wrkslots/examples/agentctl_liveness_probe.py, py/wrkviz/discover.py, py/wrkviz/responsiveness.py: stop reading REGISTRY/<name>/.
- Docs: py/agentctl/USER_GUIDE.md section Registry layout; LEGACY_SUPPORT_ENDS constants in both editions.
- Tests: the layout-1 fixtures (cross primary-legacy-adopted-registry, agent_watch_scale_tests, turn_runner_retirement) become migrate-layout fixtures or go.

Reminder mechanism: this issue (label due-2026-11-26). The repository has no running tick-hub of its own; the coordinator may add a one-shot tick-hub reminder in its harness.
