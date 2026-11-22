---
title: 'herdr-runtime-state-out-of-synced-config: keep herdr's session, log and socket files out of the synced config directory'
status: open
priority: 2
issue_type: task
labels:
- herdr
created_at: 2026-11-22T06:30:00-05:00
updated_at: 2026-11-22T06:30:00-05:00
---

# Description

Herdr writes session-history.json (hundreds of MB, rewritten continuously), session.json, its client and server logs and its sockets into its config directory. When that directory is synced between hosts, both hosts write the same files and the sync tool makes conflict copies (thousands so far). Only config.toml should be shared.

# Design

Find how herdr chooses those paths (flags, environment, XDG_STATE_HOME/XDG_RUNTIME_DIR support in the installed version) and how the sync tool can exclude them; propose the least disruptive move: no restart of a running herdr server (it hosts every agent pane), the change taking effect at the next natural restart, and every agent-utils tool that finds herdr's socket kept working.

# Acceptance Criteria

- A written plan with the mechanism verified on a throwaway herdr instance, approved before any change to the live hosts.

# Notes

Filed by the coordinator from a consumer's report; the owning lane refines this file as it works.
