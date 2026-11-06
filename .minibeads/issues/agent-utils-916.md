---
title: 'muse-native-session-id: agentctl start and adopt capture a Muse lane''s native session id'
status: open
priority: 1
issue_type: bug
assignee: cl-sync
created_at: 2026-11-06T01:31:20.342697754+00:00
updated_at: 2026-11-06T01:31:20.342697754+00:00
---

# Description

`agentctl start` doesn't capture the native conversation ID for Muse lanes: the record's session value is null. `agentctl revive NAME --archived` then fails with "record has no native conversation ID", unless someone runs `agentctl bind-session NAME <uuid>` by hand, where the uuid is the `~/.local/share/muse/sessions/<date>/<uuid>/` directory name. This blocks the owner's per-PR shepherd suspend and resume design in a consumer team.

# Design

- At start (and at adopt), capture the Muse session uuid: Muse's own session id if it exposes one, otherwise the newest session directory created after launch whose recorded process matches the pane's Muse pid. Refuse to guess between several candidates; record unknown and say so.
- Verify that Claude and Codex records carry their native ids too, and add tests where they are missing.
- `revive --archived` uses the captured id with no manual bind.

# Acceptance Criteria

- Test: start a Muse agent (a fake Muse that writes a session directory), archive it, then `revive --archived` succeeds with no bind-session.
- Test: with two candidate session directories, the record says unknown rather than picking one.
- Claude and Codex start and adopt paths are checked for native ids, with a test for each.

# Notes

Requested by the lead on 2026-11-05 at 9:02 PM ET (P1), from a consumer team. Assigned to cl-sync, after #915.
