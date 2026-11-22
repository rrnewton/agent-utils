---
title: 'agentctl-native-session-cross-binding: revive and adopt must refuse a native session already registered to another name'
status: open
priority: 1
issue_type: bug
labels:
- agentctl
created_at: 2026-11-22T12:35:00-05:00
updated_at: 2026-11-22T12:35:00-05:00
---

# Description

On a consumer host, a recovery of one review seat's lost pane bound its name to another seat's native Codex session. A later revive of the first name failed with "native session is already registered as <other>; it was recorded for <first>". So two seats meant to be independent reviewers may have been one session for several reviews, which breaks review independence.

# Design

Every path that binds a name to a native session (revive, adopt, restore, any pane recovery) refuses when that native session id is already recorded for a different live or archived name, naming both; no silent rebind. Add a registry check (agentctl doctor or a dedicated command) that lists every native session id recorded under more than one name, with the time each binding was made, so past cross-bindings can be found. Identify which agentctl version or path performed the silent bind.

# Acceptance Criteria

- Tests: revive and adopt refuse a cross-binding; the registry check reports a planted duplicate; the path that bound silently is named and fixed.

# Notes

Reported by a consumer team; filed by the coordinator.

Refined 2026-11-22 by the fixing lane:

- Silent-bind path named: `conversation()` in `rs/agentctl/src/subagents/revive.rs` fell back to on-disk discovery when a record named no native session. Codex discovery (`discover_codex_in`) picked the newest rollout naming the record's cwd and Claude discovery (`discover_claude_session`) the first matching transcript, with no ambiguity refusal (Muse discovery already refused several). The duplicate-owner check behind revive (`finish_launch_plan` -> `identity_owner` in `rs/agentctl/src/subagents.rs`) scanned only live registrations and pending revives, never `archive/`, so an archived seat's session was invisible and got asserted under the revived name. `guard_duplicate_capture` (pane recovery at start/adopt/promote) also exempted any owner that was not `running`.
- Fix: `identity_owner` also scans archived generations; Codex and Claude discovery return `Many` on more than one same-cwd session and revive refuses naming the candidates; `finish_launch_plan` refuses a merely discovered (unrecorded) session when another live or archived seat shares the cwd, naming both seats; `guard_duplicate_capture` drops the running-only exemption. Restore reaches the same checks through revive. No persisted-format change: no new record fields (binding time in the doctor report is the existing `created_at`); doctor output only gains `native_session_duplicates`, so older readers are unaffected.
- Registry check: `agentctl doctor` lists under `native_session_duplicates` every native session id recorded under more than one distinct name (live or archived), each binding with name, agent, session, source (`observed`/`asserted`), `recorded_at`, archived flag, lifecycle and cwd; a non-empty list makes the report not clean.
