---
title: 'agentctl-revive-missing-pane-dead-proof: revive a previous-boot lane whose pane is gone, refuse a same-boot missing pane with the remedy named'
status: open
priority: 1
issue_type: bug
labels:
- agentctl
- revive
created_at: 2026-11-20T05:00:00-05:00
updated_at: 2026-11-20T05:00:00-05:00
---

# Description

[watermelon-full-preview] A consumer's Claude seat could not be
revived after the reboot: `agentctl revive NAME` returns rc 69 with
"pane process-info wJ:p12C: pane_not_found" (record lifecycle
running, harness claude, created before the reboot). A Codex seat
whose failure was "agent get: agent_not_found" revived fine.

# Design

- A recorded harness identity whose boot_id differs from the current
  boot is proven dead (the boot changed, so every process of that
  boot is gone), whatever the pane lookup says; revive proceeds,
  launching a new pane.
- A missing pane alone (same boot) is not proof (pane ids and
  processes can outlive a pane mapping), so revive keeps refusing
  there, but the refusal names the supported remedy (the retire
  command, or revive with an explicit override if one exists).
- `stop --retire-dead-native` accepts a record whose pane is gone,
  so a replaced seat's stale record can be archived.
- The same applies when the Herdr agent object outlived its pane
  after a reboot: retire currently refuses with "the recorded Herdr
  agent still exists" on pane_not_found. A previous-boot harness
  identity plus a missing pane lets both retire and revive proceed
  even if the stale Herdr agent object remains; clean it up, or
  ignore it (coordb, consumer case, 2026-11-20).

# Acceptance Criteria

- A previous-boot identity plus a missing pane revives into a new
  pane.
- A same-boot identity plus a missing pane refuses, with the remedy
  named in the refusal.
- `stop --retire-dead-native` archives a record whose pane is gone,
  including when a stale Herdr agent object remains for it.
- Tests cover both boot cases.

# Notes

[watermelon-full-preview] Lane mu-restorefix/1944, queued after
#1934 agentctl-restore-muse-sessions-and-timeout and #1939
agentctl-codex-replace-goal-picker, coordinated by coordb.
