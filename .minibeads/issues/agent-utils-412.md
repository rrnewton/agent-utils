---
title: 'herdr-run-idle-ttl: retire finished command tabs after an idle TTL'
status: open
priority: 2
issue_type: feature
labels:
- herdr-run
created_at: 2026-10-22T02:50:57.716251599+00:00
updated_at: 2026-10-22T02:50:57.716251599+00:00
---

# Description

[muse watermelon] Command workspaces fill to max_panes (32) idle shells and stay there, because tabs are only closed by LRU replacement at the cap. Under load the terminal server held 123 panes, 64 of them idle shells left by finished commands. Add an idle TTL: new config idle_tab_ttl_seconds (default 900, 0 disables). A tab whose last recorded run finished more than the TTL ago and which is provably idle by the existing eviction rules (pane lock free, shell alone in the foreground, no other process in the session, one pane per tab, verdict taken under the lock right before closing) is closed. Enforcement is opportunistic at the end of every run with a small fixed work budget, plus an explicit sweep usable from a timer with --dry-run. Tabs with no run record keep today's reap handling. Both the Python and Rust editions must behave identically, with cross-language differential cases.

# Acceptance Criteria

TTL expiry closes an idle finished tab; a tab running a command, holding a background job, locked by another runner, or with two panes is never closed; TTL 0 disables; boundary timing is tested in both editions and the differential; config, CLI help and docs describe idle_tab_ttl_seconds.
