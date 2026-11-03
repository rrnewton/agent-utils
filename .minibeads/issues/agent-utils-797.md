---
title: 'wrkslots-audit-lock-free-planning: plan and measure caches outside the shared state lock'
status: closed
priority: 2
issue_type: feature
assignee: mu-rust@devbig030
created_at: 2026-11-02T23:54:46.248089304+00:00
updated_at: 2026-11-03T08:38:47.359550453+00:00
closed_at: 2026-11-03T08:38:47.359550272+00:00
---

# Description

[opus 5.5] `wrkslots audit` takes one shared `ACTIVE` lock and holds it through state reading, cache-glob planning, the cache census and row building. That gives the payload one consistent snapshot, including the registry revision it publishes. Writers default to `--wait-lock 0`, so every create, remove or adopt attempted during an audit is refused. A consumer's lock was heavily contended on 2026-11-02 (an audit needed 17 tries). Since the per-subject planning wall, planning can take up to `--cache-plan-total-seconds` (default 30 s) of that hold.

Wanted: take the registry snapshot under a short lock, then plan and measure outside it. Before publishing, revalidate that the snapshot is still current (registry revision unchanged); otherwise report the affected rows as unknown or rerun. Both editions (Python oracle and the Rust `wrkslotsd` audit) must move together, with the differential tests kept green.

Acceptance: the lock hold no longer grows with planning or census time, writers stop being refused for the length of an audit, and a registry change during the audit is detected rather than published as current.
