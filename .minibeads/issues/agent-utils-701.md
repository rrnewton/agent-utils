---
title: 'agentctl-services-refresh: guided rebuild and restart of a team''s long-running agentctl services'
status: in_progress
priority: 1
issue_type: feature
created_at: 2026-10-29T21:50:56.998638483+00:00
updated_at: 2026-10-29T21:51:05.769559311+00:00
---

# Description

[opus 5.5] A guided refresh flow for a team's long-running agentctl services, run from actual
compiled binaries of a normal release build: no launcher, no rebuild on call, no caching layer.

Agents call short commands through the checkout's caching launcher. Long-running services (chat
bridges `agentctl chat run`, watchers `agentctl watch-agents`, headless wrappers `agentctl
run-headless`) must instead run a fixed binary: they load it once, restart on crash, and spawn
helpers (the Slack plugin, the outbound helper) that must match it. Today an operator refreshes
them by hand: a clean `git archive` build, a copy into a versioned directory, a plugin home with
its manifest, a backup of each unit, `systemd-run` again with the same arguments, a health check,
and rollback notes.

`agentctl services`:

- `services list`: discover the registry's services (systemd user units whose ExecStart runs
  agentctl with this registry: chat run, watch-agents, run-headless), with binary path, the
  commit the binary reports, plugin home, state directories, and whether each is on the newest
  snapshot.
- `services build [--from CHECKOUT]`: `cargo build --release --locked` of agentctl and
  chat-subscription-slack from a clean `git archive` of the checkout's HEAD, with
  AGENTCTL_SOURCE_COMMIT set, into a versioned snapshot directory (one subdirectory per snapshot)
  holding the binaries, the Slack plugin and its manifest.
- `services refresh [--only NAME] [--dry-run]`: one service at a time, back up its unit
  definition, write rollback commands, restart it onto the snapshot keeping every argument and
  environment variable (systemd-run for transient units, a drop-in for unit files), health-check
  it (bridge: `chat health` not DOWN and the event stream fresh; watcher: a pass completes;
  headless: the wrapper answers), roll back automatically on failure, and print old -> new.
- Help, userguide, a section in common/docs, and tests with fake systemd units.
