---
title: 'team-restore: one idempotent restore command per team from a desired-state spec'
status: closed
priority: 1
issue_type: epic
assignee: mu-restore@devbig030
labels:
- agentctl
created_at: 2026-11-02T16:13:53.489220659+00:00
updated_at: 2026-11-03T17:08:55.240079751+00:00
closed_at: 2026-11-03T17:08:55.240079611+00:00
---

# Description

[opus 5.5] From the 2026-11-02 host outage (a filesystem went read-only, two hard reboots, every agent died; restore was done by hand on two hosts). Owner goal: ONE idempotent command per team, safe to run any number of times against a fresh or existing herdr, with or without restored tabs, driven by a declarative spec of what should be running, run automatically after boot (except the certificate step).  This epic tracks the gap list (25 gaps found during the restore) as issues #741-#747. Out of scope here: persistent host journal (host setup). The hung-TUI health check (working by title with a blank grid) is tracked separately.

# Notes

## Resolution

[muse watermelon-full-preview] All seven children are delivered and closed on main: #741 restore-native-sessions (`373e4905`), #742 restore-team-spec (`8fed287f`), #743 restore-installed-units (`45a8b0bb`), #744 restore-pane-reuse (`9a0a8b24`), #745 bridge-gap-backfill (`5bd86629`), #746 restore-im-back (`623493bb`), and #747 agentctl-handover (`c06dea2a`). A team now restores with one idempotent command from its spec: the certificate is checked first, Herdr and wrkslots resolve from the team folder or profiles (never PATH), only the spec's agents revive, Herdr-restored shell panes are reused or retired with re-anchors recorded, persistent units (bridges, watcher, boot restore, and spec-named scheduled timers with `ConditionHost`) replace transients, and each restore posts the deterministic "I'm back" summary with manager-first catch-up. Follow-ups left open deliberately, each in its own issue if it ever bites: spec version pins for wrkslots/agentctl (#742 resolution) and the hung-TUI health check (tracked separately, per the description).
