---
title: "watch-agents-archived: watch-agents reports archived registry records as live agents needing attention"
status: open
priority: 1
issue_type: bug
created_at: 2026-11-09T19:30:00.000000000+00:00
updated_at: 2026-11-09T19:30:00.000000000+00:00
---

# Description

A consumer coordinator recreated its `agentctl watch-agents` watcher on a recent agent-utils build and the watcher immediately flagged agents that exist only in the registry archive:

- Several agents reported as "idle >5 min without a verdict" even though `agentctl status` for those names says "unknown agent" (not in the live registry).
- One agent reported as "unreachable while its record says running", also only present under the registry's `.agentctl/archive/` directory. That archived record still carries lifecycle `running`; other archived records carry lifecycle `stopped`.

The previous watcher build did not report these archived records, so this is a regression in how `watch-agents` enumerates agents. No wrong action was taken; the cost is notice noise that erodes trust in the watcher.

Expected behaviour: an archived record is terminal whatever its lifecycle field says. `watch-agents` must never report archived records. Separately, archiving should leave no record pretending to be `running` (either archiving normalises lifecycle to `stopped`, or readers treat archive membership as terminal — preferably both).

# Design

- Find where `watch-agents` (and its single-pass health sibling) enumerates agents in each edition that implements it (Rust `rs/agentctl` and Python `py/agentctl` where present), and exclude the archive from that enumeration. The live registry is the source of truth; the archive directory is history, not membership.
- Identify the commit in the recent registry-layout / session work that started including archive records (candidates between the last good and first bad builds touch the registry layout, `AgentRecord`, the locked registry, native-session sharing, and the watch pass's auto re-pin). Name the commit and function in the fix.
- Decide whether the archive writer should normalise lifecycle to `stopped` when archiving. If safe, do it, so a stale `running` in the archive cannot mislead any other reader; keep reader-side exclusion regardless (defence in depth).
- Keep the change minimal and symmetric across editions where the code exists.

# Acceptance Criteria

- A regression test builds a registry with archived records (one with lifecycle `stopped`, one with lifecycle `running`) plus live records; the watcher reports only the live ones. The test fails on the regressing code and passes after the fix.
- `watch-agents` (and the health pass, where it shares enumeration) never reports an agent whose only record is under `.agentctl/archive/`, regardless of the archived lifecycle value.
- If archiving is changed to normalise lifecycle, a test or check shows a newly archived `running` record reads back as `stopped` (or the reader contract is documented and tested instead).
- Full validation gate passes for the touched areas in both editions where the code exists.

# Notes

Reported 2026-11-09 by a consumer coordinator via the lead coordinator. Notice noise only; no wrong action was taken. Builds compared: last good watcher build versus the recreated watcher build on the recent registry-layout series. Reproducer fixtures can be copied read-only from a live registry's archive (never edit a live registry).
