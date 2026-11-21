---
title: 'agentctl-headless-single-start-path-console-tab: one start path ensures the Herdr console tab for every headless agent'
status: closed
priority: 0
issue_type: feature
labels:
- agentctl
created_at: 2026-11-20T16:46:01.334275441+00:00
updated_at: 2026-11-21T08:56:30.902631473+00:00
closed_at: 2026-11-21T08:56:30.902631343+00:00
---

# Description

[watermelon-full-preview] Owner (238/239, 2026-11-20): headless+console must be equivalent to a normal TUI agent. It should always start with its Herdr pane and console follower; the headless server may run outside Herdr for durability, but the code path that starts it must be the same that starts the Herdr tab watcher, so headless and non-headless agents behave the same wrt Herdr tabs. It cannot be on an agent to remember. The console is interactive (agentctl console NAME, analogous to agentterm -s; typing messages to headless agents with our own minimal TUI), never the old read-only tail --follow.

Root cause (mu-headlesstab investigation, 2026-11-20): after the 11-19 reboot, lead-014 had a blank-shell viewer tab. (1) agentctl-headless-lead-014.service is a current install-units --headless unit (not legacy/hand-made), enabled WantedBy=default.target; systemd started it at boot (ActiveEnterTimestamp 2026-11-19 20:19:08 PST) outside restore. (2) Its ExecStart runs snapshot binary 91cae071 (2026-11-17), which predates the #1879 landing bd5cb785 (2026-11-19), so the running wrapper predates the console-viewer flow. (3) Even on current code, run_headless_with_effort only ensures a viewer when record.viewer_tab() is none, once, swallowing errors: after a reboot the record still names the dead pre-reboot tab, Herdr may not be up yet, and nothing ever retries; a Herdr-restored blank shell (recorded kind console, tab live, pane an idle shell) is likewise never healed. ensure_headless_viewer itself unconditionally creates a new tab, so it cannot be called blindly without duplicating a live console.

Invariant to build: every path that starts or restarts a headless agent (start, run-headless, the systemd unit ExecStart/Restart, restore --boot, revive, turnover, takeover/handback) goes through ONE function that also ensures its Herdr tab running agentctl console NAME, idempotently: create if missing, replace if blank/dead/tail, leave a live console alone. The wrapper itself ensures its own console tab at startup and whenever Herdr (re)appears, so no caller can forget; Herdr not being up must not block the agent (retry while the wrapper lives). #1971 agentctl-tabs-scheduled-hygiene owns periodic tidying/ordering only; this issue owns console-tab creation for headless agents.

Recipe for the affected consumer team: bump the snapshot binary their unit ExecStart points at (services turnover / install-units) to a build containing this fix, restart the unit, and the wrapper recreates the console tab itself; no unit-shape conversion is needed because ExecStart already runs run-headless.

# Design

One ensure function (ensure_headless_console_tab) in subagents/control.rs: inspect recorded viewer tab against live panes; live console (kind console, live pane, not an idle shell) untouched; tail/blank/dead/missing replaced or created as console in the viewer workspace; record viewer_tab_id+kind console. start_headless and hygiene CreateViewer call it; run_headless_with_effort spawns a scoped viewer thread that calls it at startup and retries on an interval until the wrapper stops, so the systemd unit path, restore, revive, turnover and handback (all of which relaunch the wrapper) inherit it without remembering a separate step.

# Acceptance Criteria

Fake-Herdr/scratch-registry tests: killed viewer comes back; unit-path (run-headless) start recreates it; restore-after-reboot shape (stale recorded tab, blank shell) heals to console; a tail viewer is replaced by a console; a live console is untouched with no duplicates. Herdr down at wrapper start does not block the agent and the tab appears when Herdr appears. Focused suites pass under nice -n 10; one gate run; cold review by coordb of the SHA before gating (tab creation/replacement is closing-adjacent). Recipe written for the affected consumer team.

# Notes

## Resolution

[watermelon-full-preview] Landed on main as 3afebc51 (tree 43fc5a33203fee09c19a18d7fadc71e6c5428760), a verified-tree landing under the coordb gate waiver for the #2027 pidfd pair and the #2024 console_pty artefact. One idempotent ensure path now creates/replaces/leaves the console tab from every headless start, with the wrapper retrying while it runs; the consumer recipe is in the headless userguide.
