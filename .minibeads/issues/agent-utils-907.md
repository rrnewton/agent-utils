---
title: 'gate-blackout-window: gate.py refuses to start a gate inside a configured per-host blackout window'
status: open
priority: 2
issue_type: feature
assignee: mu-validate
created_at: 2026-11-05T18:19:37.637194904+00:00
updated_at: 2026-11-05T18:19:37.637194904+00:00
---

# Description

The gate host is shared. Another team runs a heavy nightly job on it from 01:00 to 04:00 ET, and asked on 2026-11-05 that no agent-utils gate start between 00:15 and 04:00 ET. Today that rule lives only in messages to lanes. Agents forget, and the tool should not.

# Design

- A per-host blackout window for `scripts/gate.py`. One or more windows, each with a start time, an end time and a time zone (e.g. 00:15-04:00 America/New_York), plus an optional reason string.
- Read the windows from a per-host config file that is NOT synced between hosts. Prefer `~/.local/etc/agent-utils/gate.toml` or `~/.local/state/...`, since parts of `~/.config` are synced across the owner's hosts. An environment variable may override the path. With no file, there is no blackout.
- During a window, gate.py refuses to START a gate (normal or `--exclusive`). The message names the window, its reason and the next allowed start time in the window's time zone, and exits with a distinct status. A queued gate whose turn comes during a window waits, or refuses, consistently with the start rule; document which.
- `--ignore-blackout` (or similar) overrides, and the override is logged in the gate ledger.
- Running gates are never touched.

# Acceptance Criteria

- Test with an injected clock: inside the window a start is refused, with the next allowed time; outside it the gate starts; a window crossing midnight works; `--ignore-blackout` starts and is recorded in the ledger.
- Test: no config means no blackout, and a malformed config refuses with a clear error rather than silently ignoring the file.
- `gate.py --help` documents the config path, the format and the flag, with an example.

# Notes

Requested by the lead on 2026-11-05 at 1:19 PM ET (P2, small), after a neighbouring team on the shared gate host asked for quiet hours during its nightly run. Assigned to mu-validate (gate tooling, #827) after #898. Until it lands, lanes have been told not to start gates between 00:15 and 04:00 ET.
