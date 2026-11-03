---
title: 'restore-installed-units: persistent user units instead of transients, installed from the spec'
status: closed
priority: 1
issue_type: feature
assignee: mu-restore@devbig030
labels:
- agentctl
depends_on:
  agent-utils-740: parent-child
created_at: 2026-11-02T16:13:53.501647376+00:00
updated_at: 2026-11-03T17:08:09.263663630+00:00
closed_at: 2026-11-03T17:08:09.263663500+00:00
---

# Description

[opus 5.5] From the 2026-11-02 host outage (a filesystem went read-only, two hard reboots, every agent died; restore was done by hand on two hosts). Owner goal: ONE idempotent command per team, safe to run any number of times against a fresh or existing herdr, with or without restored tabs, driven by a declarative spec of what should be running, run automatically after boot (except the certificate step).  Every chat bridge, watcher and timer was a systemd-run transient and vanished on reboot; they were recreated by hand from a services ROLLBACK.sh. install-units writes persistent user units (bridges, watchers, herdr server, boot-time team-restore waiting for the certificate) from the team spec; services refresh updates them in place; team-restore starts any that are missing.

# Notes

## Resolution

[muse watermelon-full-preview] Delivered on main: persistent bridge units via `install-units --bridge NAME` (earlier slice, noted in the comments), the Herdr server, watch and boot-restore units from `install-units`, `services refresh` updating unit files in place, and restore starting configured units that are missing. The remaining timer piece landed in `a3e3ec97`: the team spec (`restore.json`) names `scheduled` entries (`{name, on_calendar, command}`, validated as unique slug names, one literal calendar line, and an absolute program), and `agentctl install-units --scheduled` renders each as a persistent `agentctl-scheduled-NAME.service` plus `agentctl-scheduled-NAME.timer` user unit, both with `ConditionHost` set to the installing host and the timer `Persistent=true`, so no scheduled job lives in a `systemd-run` transient. Nothing is enabled or started by the install; the output lists the `systemctl --user enable --now` commands. Tests: `scheduled_entries_load_and_are_validated`, `scheduled_pair_is_persistent_host_bound_and_fully_rendered`, `scheduled_rendering_refuses_unrenderable_entries`, `install_scheduled_writes_every_spec_pair_and_nothing_else`; gate `validate: OK` (2010 Rust cases, 104+186+383+223 differential checks).
