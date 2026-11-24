---
title: 'agentctl-restore-headless-viewer-tab: restore proves each headless agent''s viewer tab is live, and recreates a dead one'
status: open
priority: 2
issue_type: bug
labels:
- agentctl
- restore
- headless
created_at: 2026-11-24T12:33:00-05:00
updated_at: 2026-11-24T12:33:00-05:00
---

# Description

After a host reboot, a headless coordinator came back running (its wrapper unit resumed the session), but the Herdr tab recorded as its viewer (`viewer_kind: console`, `viewer_tab_id` set) held only an empty shell. The owner opened the tab and saw nothing, so it looked as if the agent was gone.

Cause: the wrapper ran a services snapshot older than #1973. On that build, nothing re-ensures the console tab after a restart, and restore does not check viewer tabs at all. On current builds, the wrapper's start path ensures the console tab, but restore itself still never proves the viewer is live. Any wrapper on an older snapshot, or any ensure that fails while the wrapper runs, leaves the owner with an empty tab and no report.

The interim fix was to run `agentctl console NAME` by hand in the recorded tab.

# Design

- Restore, and `restore --dry-run`, list every headless agent whose record names a viewer tab. For each one, prove that the tab exists and that its pane runs `agentctl console NAME` or `agentctl tail NAME --follow` for this registry. An idle shell, a missing tab, or a viewer of a different agent counts as dead.
- A dead viewer is recreated through the same idempotent ensure path #1973 uses (create, replace or leave), in the recorded tab when it exists, otherwise in a new tab that is then recorded. This never starts a second harness process for the session.
- Report the outcome per agent: live, recreated, or failed with the reason.
- When a headless wrapper runs a snapshot older than the viewer-ensure path, restore says so and names the refresh command, so the operator can move it at a safe point.

# Acceptance Criteria

- Test: a headless agent whose recorded viewer tab holds an idle shell is reported dead by `restore --dry-run`. A real restore leaves the tab running `agentctl console NAME`, and the record still names that tab.
- Test: a live console viewer is left untouched; no pane is replaced and no second viewer is created.
- Test: a missing viewer tab is recreated and recorded. A viewer showing a different agent is not counted as live.
- No harness process is started or restarted by the viewer check.

# Notes

Seen on 2026-11-24 after a fleet-repair reboot. Related: #1973 (the headless start path ensures the console tab), #1971 (scheduled tab hygiene, headless viewer recreation), #1793 (workspace cultivation, headless viewers).
