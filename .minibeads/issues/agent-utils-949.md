---
title: 'restore-muse-lanes: agentctl restore should attempt working Muse lanes that have recorded sessions'
status: open
priority: 1
issue_type: feature
created_at: 2026-11-06T18:16:13.000000000+00:00
updated_at: 2026-11-06T18:16:13.000000000+00:00
---

# Description

After a reboot, `agentctl restore` did not even try the working Muse lanes, because restore.json's desired-state agent list did not include them. The list is written by `restore --write-agents --apply`, so it is only as current as the last time someone ran that, and lanes started later are left for hand recovery.

# Design

- Design note first, in ai_docs/transient/: decide whether restore should consider every live record with a launch spec and a recorded native session, beyond the agents restore.json lists. restore.json's `exclude` would still win.
- Alternatively, or in addition, keep the list current automatically: start and retire update the restore.json agents list, or warn when a running agent is missing from it.
- Revive order and the existing safety checks are unchanged; this issue changes only which agents restore attempts.

# Acceptance Criteria

- The design note is written and a choice is made.
- A test: a live Muse lane with a recorded session that was started after the last write-agents run is attempted by restore (or the lane is added to the list at start), and an excluded lane is not attempted.

# Notes

Reported 2026-11-06 by the lead coordinator during a reboot recovery. Depends in practice on #946 and #948, because restore uses the same revive path.
