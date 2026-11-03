---
title: 'restore-pane-reuse: reuse or retire herdr-restored shell panes; re-anchor restarted harnesses'
status: closed
priority: 1
issue_type: feature
assignee: mu-restore@devbig030
labels:
- agentctl
depends_on:
  agent-utils-740: parent-child
created_at: 2026-11-02T16:13:53.505795107+00:00
updated_at: 2026-11-03T13:37:49.561218255+00:00
closed_at: 2026-11-03T13:37:49.561218125+00:00
---

# Description

[opus 5.5] From the 2026-11-02 host outage (a filesystem went read-only, two hard reboots, every agent died; restore was done by hand on two hosts). Owner goal: ONE idempotent command per team, safe to run any number of times against a fresh or existing herdr, with or without restored tabs, driven by a declarative spec of what should be running, run automatically after boot (except the certificate step).  herdr restores saved tabs as plain shells, and revive always created new tabs (47 idle shells beside 7 live agents on one host). Owner-endorsed rule: reuse a pane only when its only foreground process is a plain shell with no children and its pane id, tab label or cwd matches the record; otherwise create a new tab and close the idle duplicate. A dead Muse record could not be stopped while its pane was a plain shell ('custom harness muse is not the foreground process'; --retire-dead-native refused): treat that as harness exited. Same pane plus same harness plus a new PID must re-anchor registry and bridge records automatically (a coordinator restart queued messages 15 min). wrkslots: re-acquire a released generation for the same agent name and slot.

# Notes

## Resolution

[muse watermelon-full-preview] Delivered on main: `75c7de9e` records re-anchors in agent.json (reason `restart-same-pane`, previous/current pane, terminal and pid, slot, `slot_generation` null); `cf0e14d1` retires a dead custom record whose pane is a plain shell; `2e5ca138` revive reuses a proved descendant-free idle shell pane that reports no session for custom (herdr-pane/herdr-relay) records, keeps the pane and tab, records `reason=reuse`, and recovers after interruption, while panes still reporting a session keep the census-exclusion path (new tab, idle duplicate closed). The wrkslots classification of re-registered panes is #777 (closed). Tests: `revive_reuses_a_proved_idle_shell_pane_and_records_the_reanchor`, `revive_reuse_recovers_after_an_interruption_without_a_new_tab`, anchor re-anchor identity tests; gate `validate: OK` (1993 Rust cases, 104+186+383+223 differential checks).
