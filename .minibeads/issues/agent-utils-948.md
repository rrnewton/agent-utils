---
title: 'agentctl-muse-native-session-missing: a Muse lane started after #916 had no native session recorded, so revive refused'
status: open
priority: 1
issue_type: bug
created_at: 2026-11-06T18:16:13.000000000+00:00
updated_at: 2026-11-06T18:16:13.000000000+00:00
---

# Description

A Muse lane started on a harness checkout that already contains #916 muse-native-session-id had no native conversation id in its record. After a reboot, `agentctl revive` refused it with "record has no native conversation ID". The coordinator found the session by searching Muse's session directory for the lane's slot path.

# Design

- Find why #916's capture missed this start, for example a timing window or a start path that skips the capture, and fix the capture.
- Revive fallback: when the record has no native id, look in Muse's session store for sessions whose working directory is the record's cwd or slot. Use the match only if exactly one is found, record it, and say so in the revive result. With zero or several matches, refuse and list the candidates.
- Make this case visible before a reboot needs it: status/doctor flags a live Muse record that has no native id.

# Acceptance Criteria

- A test reproduces the missed capture and shows it fixed.
- A test revives a record with no native id through the single-match fallback; zero and several matches are refused with candidates listed.

# Notes

Reported 2026-11-06 by the lead coordinator during a reboot recovery. Related: #916 muse-native-session-id, #928 native-session-collision, #946, #947.
