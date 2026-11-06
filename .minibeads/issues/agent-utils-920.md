---
title: 'revive-repeat-incomplete: repeated stop/revive ends incomplete and the printed recovery refuses'
status: closed
priority: 1
issue_type: bug
assignee: cl-sync
created_at: 2026-11-06T03:45:00+00:00
updated_at: 2026-11-06T12:42:29.919501359+00:00
closed_at: 2026-11-06T12:42:29.919501209+00:00
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

## Resolution

The incomplete revives came from readiness, not from leftover state. A launching Muse counted as ready only while the visible rows still showed its `Muse Code 1.4.x` header. A resume reprints the whole conversation, and each one after an unclean end adds a warning, so by the third consecutive revive the header had scrolled off while Muse sat idle: the launch failed and the revive ended incomplete. The printed rerun then refused because the failed candidate had never been anchored (no terminal anchor, since anchoring follows readiness).

- A launching Muse is ready at an idle editor without its header while its pinned process verifies on the same poll. #946 agentctl-muse-resume-readiness landed the same readiness change first; this keeps its test of a screen after three resumes.
- Rerunning `agentctl revive NAME`, as printed, anchors a launch-failed candidate whose harness still runs in the candidate's own pane, and finishes.
- `stop` asks a Muse lane's process tree to exit (SIGTERM, runtime first) before closing its pane, so Muse records a clean `session_end`; checked live: closing the pane left no `session_end` and the next resume warned "ended abnormally"; SIGTERM to the runtime wrote `exit_reason: clean`. A TERM to the pinned wrapper alone orphaned the runtime, which `restart_with_resume` did; it now ends the whole tree too.
- Tests: five stop/revive cycles each for Claude, Codex and Muse; stop asks Muse to exit; an incomplete revive recovered by running the printed command verbatim (Claude and Muse); a Muse screen after three resumes with its header scrolled off.

Open: the benchmark's Codex start-to-ready waits of about 309 s and 485 s, as seen through `agentctl status`, were not reproduced (Herdr 0.9.5 reads Codex idle from its title here). They need that host's Codex screen at the time; file a new issue with it if they recur.
