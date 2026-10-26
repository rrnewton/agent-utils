---
title: 'herdr-run-idle-retention: keep few idle command tabs, reuse the least recently used one'
status: open
priority: 2
issue_type: feature
labels:
- herdr-run
created_at: 2026-10-26T00:35:46.608143478+00:00
updated_at: 2026-10-26T00:35:46.608143478+00:00
---

# Description

[opus 5.5] Command workspaces still accumulate idle tabs up to max_panes: on 2026-10-25 one workspace held 32 tabs (its cap), 30 of them idle far past idle_tab_ttl_seconds, because the TTL sweep runs only for a bounded number of tabs when a run finishes and nothing sweeps when runs are rare; the terminal server held 148 panes, sat near one full core and timed out control calls. Add an idle-retention cap separate from max_panes: max_idle_tabs (default 3, 0 disables) idle command tabs per workspace, closing the least recently used idle ones after every run and from sweep --idle; tabs running commands are never counted or closed. When a new agent needs a tab and the idle pool is already full, rename the least recently used provably idle tab for it instead of creating one, keeping the pane lock through the run. Shorter default idle_tab_ttl_seconds (300), and the full TTL sweep on every run completion. Measure control-call latency and server CPU against tab count.

# Acceptance Criteria

Both editions: at most max_idle_tabs idle recorded tabs remain after a run or sweep; a busy, locked, split or in-flight tab is never counted or closed; a new agent reuses the LRU idle tab only when the pool is full; the reused pane's lock is held from the idle verdict through the command; sweep JSON names the cause of each close; config, init template, CLI help, quickstart and userguide describe max_idle_tabs and a sweep timer recipe; unit, golden and differential e2e cases; a benchmark of tab count vs Herdr latency/CPU with conclusions.
