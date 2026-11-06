---
title: 'revive-repeat-incomplete: repeated stop/revive ends incomplete and the printed recovery refuses'
status: open
priority: 1
issue_type: bug
assignee: cl-sync
created_at: 2026-11-06T03:45:00.000000000+00:00
updated_at: 2026-11-06T03:45:00.000000000+00:00
---

# Description

A consumer ran a session-resume benchmark: repeated `agentctl stop` then `agentctl revive` on one agent.
- Claude: 6 of 6 OK, median 8.34 s.
- Muse: 6 of 8 OK, median 6.50 s. In 2 of 2 runs the revive ended "incomplete" on the third consecutive stop/revive.
- Codex: 2 of 3 OK. It ended incomplete on the first cycle; its readiness is tracked in #921.

`agentctl stop` leaves a Muse session "ended abnormally", which probably trips the readiness check on the next revive.

Recovery took closing the pane and then `retire-stale-registration` with the token, the record sha and the revive sha. The recovery command that agentctl PRINTS refuses as printed.

A missing native session id is a separate problem (#916 muse-native-session-id).

# Design

- `agentctl stop` ends a Muse session with a graceful exit, not a kill, so the next revive sees a clean end.
- The recovery command printed after an incomplete revive must work exactly as printed, filling in the token and shas it needs.
- Find out why the third cycle fails: leftover state, readiness or the abnormal end.

# Acceptance Criteria

- A test runs at least five consecutive stop/revive cycles per harness that supports revive (fake harnesses are fine), and every cycle completes.
- After stop, a Muse session is reported as ended normally.
- A test drives an incomplete revive and runs the printed recovery command verbatim, and the command succeeds.

# Notes

Reported 2026-11-05 10:40 PM ET by the lead coordinator, from a consumer's revive benchmark; the exact failure text is in that benchmark's report.
