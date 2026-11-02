---
title: 'restore-native-sessions: record and discover every harness''s native session id'
status: closed
priority: 1
issue_type: feature
assignee: mu-restore@devbig030
labels:
- agentctl
depends_on:
  agent-utils-740: parent-child
created_at: 2026-11-02T16:13:53.493499578+00:00
updated_at: 2026-11-02T19:31:58.329874483+00:00
closed_at: 2026-11-02T19:31:58.329874333+00:00
---

# Description

[opus 5.5] From the 2026-11-02 host outage (a filesystem went read-only, two hard reboots, every agent died; restore was done by hand on two hosts). Owner goal: ONE idempotent command per team, safe to run any number of times against a fresh or existing herdr, with or without restored tabs, driven by a declarative spec of what should be running, run automatically after boot (except the certificate step).  Gaps: restore refused most agents with 'record has no native conversation ID' (3 of 7 and 30 of 31 records on two teams, all coordinators). Record the native session id at start, adopt and resume for every harness (Claude: the jsonl under ~/.claude/projects/<cwd>; Codex: the rollout file); when missing, discover it by cwd plus last activity before the outage, confirm by content, and bind it. Muse has no resume: restart fresh with an automatic handoff brief (status file, transcript tail, queued messages). Never resume a session another live process owns (use a fork). Add revive of an archived record (stop then revive currently reports 'unknown agent').
