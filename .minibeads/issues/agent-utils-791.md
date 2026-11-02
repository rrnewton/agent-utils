---
title: 'chat-health-wrong-registry: chat health resolves the bridge''s target in the caller''s registry and reports a false DOWN'
status: open
priority: 2
issue_type: bug
labels:
- agentctl
created_at: 2026-11-02T20:52:47.732833977+00:00
updated_at: 2026-11-02T20:52:47.732833977+00:00
---

# Description

[opus 5.5] 2026-11-02 on one host: 'chat health' for a consumer's bridge reported 'DOWN: target <coordinator> is not a usable registered agent: unknown agent <coordinator> ... NOT ALIVE' while 'status <coordinator>' in the bridge's own registry answered normally and the bridge was delivering (last delivered minutes earlier). Likely cause: chat health resolves the target in the caller's default registry (.agentctl near the cwd) instead of the registry the bridge runs with; with an explicit --registry it is correct. Fix: chat health takes the registry from the bridge (its unit ExecStart, or one recorded in bridge state); if a different registry is passed or defaulted, it says so (registry mismatch) instead of reporting DOWN. A false DOWN hides real outages. Test: health run from a directory with a different default registry.
