---
title: 'agentctl-restore-headless-viewer-tab: restore proves each headless agent''s viewer tab is live, and recreates a dead one'
status: closed
priority: 2
issue_type: bug
labels:
- agentctl
- restore
- headless
created_at: 2026-11-24T12:33:00-05:00
updated_at: 2026-11-26T07:45:00-05:00
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

## Resolution

Landed on main in the agentctl stack a724562b..7e89d795 (12 commits replayed by land-main from the gated 62a72cde, the same patch, in the order #2218, #2214, #2222, #2217), as 8e37076a, c8e7c88e, 5dab62ca and 295b0d55. Gate ticket 1795671224491121398-107888b7: full selection, 65 steps, pass, finished 7:32 AM ET on 2026-11-26 (rust.agentctl 3106 passed, 0 failed; both python.agentctl shards ran, 1457 passed each).

- A recorded headless viewer counts as live only when its pane provably runs `agentctl console NAME` or `agentctl tail NAME --follow` for this registry. An idle shell, a missing tab, another agent's viewer or another registry's console is dead. An unreadable foreground or a viewer created moments ago is unproven and is never touched.
- A dead viewer is recreated in the recorded tab's own idle pane when that pane is ours and alone, otherwise through the #1973 ensure path, and the record names the tab. The ensure path itself no longer calls a readable non-viewer foreground live. No harness process is started or restarted.
- `restore` and `restore --dry-run` report one `headless_viewers` row per headless agent whose record names a viewer tab: live, dead, unproven, recreated or failed. A wrapper unit whose binary lacks the viewer-ensure capability is named stale, with the services refresh command. Restore's tab tidy no longer parks a recorded viewer tab as an orphan shell.
- Tests (Fake-based, no real pane): `restore_proves_an_idle_shell_viewer_dead_and_recreates_it_in_its_tab`, `restore_leaves_a_proven_live_console_and_a_follow_tail_untouched`, `restore_recreates_a_missing_viewer_tab_and_records_the_new_one`, `a_viewer_showing_another_agent_is_not_live_and_is_never_closed`, `restore_dry_run_reports_a_dead_headless_viewer_and_a_real_restore_recreates_it_in_place`, `restore_recreates_a_missing_headless_viewer_tab_and_names_a_stale_wrapper_snapshot`.
- Persisted fields: the restore report adds `headless_viewers` rows (additive); the record's viewer fields already existed.
