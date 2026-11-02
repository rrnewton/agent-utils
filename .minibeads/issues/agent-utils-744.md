---
title: 'restore-pane-reuse: reuse or retire herdr-restored shell panes; re-anchor restarted harnesses'
status: open
priority: 1
assignee: mu-restore@devbig030
issue_type: feature
labels:
- agentctl
depends_on:
  agent-utils-740: parent-child
created_at: 2026-11-02T16:13:53.505795107+00:00
updated_at: 2026-11-02T16:13:53.505795107+00:00
---

# Description

[opus 5.5] From the 2026-11-02 host outage (a filesystem went read-only, two hard reboots, every agent died; restore was done by hand on two hosts). Owner goal: ONE idempotent command per team, safe to run any number of times against a fresh or existing herdr, with or without restored tabs, driven by a declarative spec of what should be running, run automatically after boot (except the certificate step).  herdr restores saved tabs as plain shells, and revive always created new tabs (47 idle shells beside 7 live agents on one host). Owner-endorsed rule: reuse a pane only when its only foreground process is a plain shell with no children and its pane id, tab label or cwd matches the record; otherwise create a new tab and close the idle duplicate. A dead Muse record could not be stopped while its pane was a plain shell ('custom harness muse is not the foreground process'; --retire-dead-native refused): treat that as harness exited. Same pane plus same harness plus a new PID must re-anchor registry and bridge records automatically (a coordinator restart queued messages 15 min). wrkslots: re-acquire a released generation for the same agent name and slot.
