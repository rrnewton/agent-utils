---
title: 'agentctl-tabs-scheduled-hygiene: scheduled tab hygiene, bounded dry-run, headless viewer recreation'
status: closed
closed_at: 2026-11-21T00:00:00-05:00
priority: 1
issue_type: feature
labels:
- agentctl
- tabs
created_at: 2026-11-20T11:27:00-05:00
updated_at: 2026-11-21T00:00:00-05:00
---

# Description

[watermelon-full-preview] Owner report (232, 11:27 AM ET 2026-11-20): "I don't see its coordinator in herdr -- too many tabs, not ordered, mixed with blank shells. The old lead-014 is a blank shell. ... Seems our daemon for neatening/ordering/cleaning blanks isn't working still?" Liaison findings: `agentctl tabs` (#1793 agentctl-workspace-cultivation) exists, but no timer or unit calls it; it only runs at turnover. `agentctl tabs --dry-run` on a 77-record registry hung for more than 60 s. Headless coordinators (e.g. lead-014) have only a viewer tab, and after a reboot that viewer is not recreated.

# Design

- (1) Dry-run hang: profile `tabs_listing`/`hygiene_pass` on a scratch registry with about 80 records plus a fake Herdr. Count Herdr calls per pass; every Herdr call must be bounded (per-call timeout and a pass-level bound so N panes cannot multiply an unbounded or 30 s-per-call census into an apparent hang). Reuse one `pane_info` per pane instead of duplicate calls, and avoid per-agent work that spawns subprocesses (e.g. profile drift via git) on the listing path.
- (2) Ordering: canonical order already ranks liaison first, designated coordinator (and liaison/hands roles) next, then lanes, then unmanaged (`tab_order::role_for`, `canonical_tabs`); verify the scheduled pass applies it in every managed workspace. Blank/bare shells close only under existing `close_bare_shells` rules: kernel-proven idle shell, unclaimed, empty composer, past `bare_shell_grace_seconds`. Never close a claimed pane.
- (3) Regular runs: a systemd user timer installed by `agentctl install-units` (opt-in per team, e.g. `--tab-hygiene` / scheduled entry) that runs the hygiene pass regularly, plus one hygiene run at the end of `restore`. Headless viewer creation/recreation is DROPPED from this issue (coordb 238/239, 2026-11-20): #1973 mu-headlesstab owns creating/recreating headless agents' Herdr tabs as `agentctl console NAME`, never `tail --follow`. This issue must not touch viewer creation; ordering only ranks whatever `viewer_tab_id` the console path records.
- (4) Recipe: a short enablement recipe for another team to turn this on for their own registry (install-units flag, daemon-reload/enable timer, verify with `agentctl tabs --dry-run`).
- Safety: closing panes is safety-relevant; send the coordinator the SHA for a cold review before gating. No new persisted registry fields without reporting first (a consumer pins an older agentctl).

# Acceptance Criteria

- `agentctl tabs --dry-run` on a scratch ~80-record registry with a fake Herdr completes in bounded time, with every Herdr call bounded; a fake Herdr that stalls a call produces a clear bounded failure, not a hang.
- Tabs are ordered liaison/designated coordinator/hands first, then lanes; bare shells close only under the existing grace/idle/unclaimed rules; a claimed pane is never closed (fake-based tests).
- `agentctl install-units` can install a tab-hygiene timer (opt-in); `restore` runs one hygiene pass at its end.
- A short recipe lets another team enable scheduled hygiene on their own registry.

# Notes

Scratch registries and a fake Herdr only; never touch another team's registry or panes not created for this work. Related: #1793 agentctl-workspace-cultivation, #1225 workspace-goal-reconciler, #1004 liaison-first-tab-order, #979 canonical-tab-order.

## Resolution

[watermelon-full-preview] Closed 2026-11-21, landed as `af3d9b41` on main (tree `6fc0d248`, verified-tree landing after a cold review and two re-reviews by coordb). The dry-run hang was the census, not a stuck call: per pane it paid two `pane get`s, an idle-shell proof whose stable-identity observation costs seconds on a dead or exec-ing shell, and a screen read, plus two live Herdr calls per record through `status()` — about 480 calls for 80 records. It now reads record fields from the record file (`ManagedAgents::census_record`), gets each pane once, probes only unclaimed panes (a claim counts only when verified by recorded workspace or terminal, and any by-id claim still blocks a close), and gives the census a 60 s budget after which unprobed panes are kept; the tab socket caps the replies one request will skip. `coordinator`/`coordinator-hands` record roles rank as coordinators, and a viewer tab is renamed only when its pane is an idle shell or this agent's own `agentctl tail|console`. `agentctl tabs --apply` runs one pass with grace clocks in `REGISTRY/tab-hygiene.json` (dropped on a boot change or after about two passes, under a registry flock), `restore` runs it at its end, and `install-units --tab-hygiene` installs the per-registry timer pair (every five minutes, `TimeoutStartSec=5min`); the enablement recipe is in the userguide. Headless viewer creation stayed with #1973 and is suppressed in the standalone passes. Verified: the scratch 80-record fake-Herdr dry run completes in 82 Herdr calls; gates on three successive bases red only on the #2027 pidfd family (waived by coordb; the pair passes on current main) and Python shards run directly were green apart from the pidfd twin; the verified tree passed fmt, clippy with warnings denied, 188 focused Rust tests, 315 queue/team tests and both hygiene checkers. Nothing was enabled on any live registry: each team opts in with the recipe.
