---
title: 'chat-health-wrong-registry: chat health resolves the bridge''s target in the caller''s registry and reports a false DOWN'
status: closed
priority: 2
issue_type: bug
assignee: cl-start@devbig030
labels:
- agentctl
created_at: 2026-11-02T20:52:47.732833977+00:00
updated_at: 2026-11-02T22:45:05.910199058+00:00
closed_at: 2026-11-02T22:45:05.910198918+00:00
---

# Description

[opus 5.5] 2026-11-02 on one host: 'chat health' for a consumer's bridge reported 'DOWN: target <coordinator> is not a usable registered agent: unknown agent <coordinator> ... NOT ALIVE' while 'status <coordinator>' in the bridge's own registry answered normally and the bridge was delivering (last delivered minutes earlier). Likely cause: chat health resolves the target in the caller's default registry (.agentctl near the cwd) instead of the registry the bridge runs with; with an explicit --registry it is correct. Fix: chat health takes the registry from the bridge (its unit ExecStart, or one recorded in bridge state); if a different registry is passed or defaulted, it says so (registry mismatch) instead of reporting DOWN. A false DOWN hides real outages. Test: health run from a directory with a different default registry.

# Notes

## Resolution

[opus 5.5] chat health now looks the target up in the registry the bridge runs with. That is the running service's `--registry` (resolved against that process's working directory), else the registry `chat run` recorded for the state in the bridge index, else the command's `--registry`. A differing registry is named on a line of its own and in the JSON report (`registry.given`, `bridge`, `bridge_source`, `probed`). When the bridge's registry cannot be read, a target the given registry does not know is reported as a DEGRADED registry mismatch, never DOWN. Tests: the `choose_registry` cases, the mismatch verdict, reading `--registry` from a live process's command line, and the index lookup.
