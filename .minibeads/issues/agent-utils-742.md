---
title: 'restore-team-spec: desired-state spec and an idempotent team-restore command'
status: closed
priority: 1
issue_type: feature
assignee: mu-restore@devbig030
labels:
- agentctl
depends_on:
  agent-utils-740: parent-child
created_at: 2026-11-02T16:13:53.497507497+00:00
updated_at: 2026-11-03T14:50:16.067282568+00:00
closed_at: 2026-11-03T14:50:16.067282448+00:00
---

# Description

[opus 5.5] From the 2026-11-02 host outage (a filesystem went read-only, two hard reboots, every agent died; restore was done by hand on two hosts). Owner goal: ONE idempotent command per team, safe to run any number of times against a fresh or existing herdr, with or without restored tabs, driven by a declarative spec of what should be running, run automatically after boot (except the certificate step).  A spec (restore.json or the team spec) lists who should be running: agents with tabs, launch commands, sessions, slots; bridges; units; herdr version. team-restore repairs whatever differs and leaves everything else retired (today restore revives every dead record, including finished one-off lanes). Resolve wrkslots and herdr from the team folder or profiles.json, never PATH alone (a non-login shell hung 300 s silently); start herdr if not running and print progress. The team's own pinned bin/agentctl is the only entry point; pin versioned binaries in the team folder and check versions (an in-place update left a herdr-0.8.0 file running 0.9.5). Check the user certificate first and print exactly what to run.

# Notes

## Slice: write the desired agents list (added 2026-11-02, after #747)

[opus 5.5] `agentctl restore spec --write-agents` (name open) writes or updates the `"agents"` list in `<registry>/restore.json` from the registry's currently live, non-coordinator records, so a team refreshes its desired state in one command instead of hand-editing. By default it prints a preview diff and changes nothing; an explicit flag writes it, atomically (temp file plus rename), keeping every other key unchanged. It creates a minimal `restore.json` (schema plus coordinator) when none exists. Help covers every argument with an example. Once it lands, the coordinators write restore.json for their own registries on both hosts.

## Resolution

[muse watermelon-full-preview] Delivered on main: desired-state `agents` allowlist with progress output (`288d78ad`), `restore --write-agents` preview/`--apply` (`f25c915f`) with live-state membership from #810 (`e31c3d5d`), certificate check first with the exact fetch remedy and the Herdr unit started when Herdr is down (`4b2fdd24`), a `herdr_version` spec pin reported against the resolved binary (`379c3309`), and Herdr/wrkslots resolved from the team folder or `profiles.json`, never PATH (`242cf5d5`). Units named in the spec start after the agents, and the agentctl binary itself is pinned by the build-commit staleness check against the team's checkout. The deployment follow-on is done on both hosts: one host's `restore.json` carries six lanes plus the coordinator and is kept current with `--write-agents`; the other host's carries the coordinator plus two lanes, the persistent watcher and bridge units, and a boot oneshot that restores the registry. No spec version pins for wrkslots/agentctl were added; they remain a possible follow-up if a version mismatch ever bites.
