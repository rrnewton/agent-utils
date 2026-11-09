---
title: 'repair-notify-pair: install the repair unit beside the notifier with clear names'
status: open
priority: 1
issue_type: feature
created_at: 2026-11-09T16:52:51.923846039+00:00
updated_at: 2026-11-09T16:52:51.927743175+00:00
---

# Description

Owner request 2026-11-09 (reply 244), found while reviewing the unit sets after the reboot restore. The repair pass (tab hygiene, queue drain, health and repair of the terminal server, units, timers, slot images and this boot's restore) runs only when a team has a watch unit. The notifier pass (telling a manager or the coordinator when an agent is blocked, idle, stalled, or finished) is a different command with its own unit. On one host both registries have only the notifier, so their workspaces are never tidied, queued prompts are never drained by the repair pass, and nothing repairs a stopped server or timer there. Nothing in the default install path prevents that half-installed state, and nothing flags it afterwards.

The generated names make the gap easy to miss: the repair unit and the notifier units carry near-identical watch names, so an operator listing units cannot tell at a glance which behaviour each one runs, and a team can believe repair is covered when only notification is.

Impact: a team installed through the default path can run indefinitely with notification but no repair. The failures repair exists to fix then present as unrelated stalls: orphan shells accumulate, queues sit untyped, and a dead server or timer stays dead until a person intervenes.

# Design

Make the pair the default. The default install path (the services snapshot flow and the setup skill's unit generation) installs the repair unit next to the notifier for every team, so a fresh team cannot end up with only one. Installing one without the other stays possible only as an explicit opt-out that the install output names.

Flag the half-installed state where an operator already looks: the repair check-only pass and the host restore command each report a registry that has a notifier unit but no repair unit (and the reverse), naming the missing unit and the install command that adds it. The check reads unit presence only; it installs, enables, and starts nothing.

Rename the generated units so the name states the behaviour: for example a repair name and a notify name scoped per team. Existing installed units keep working under their current names; the tool only generates the new names for new installs and prints a migration note (old name, new name, and the operator steps to rename). Live units are renamed by the host operator or coordinators, never by the tool, and the tool never deletes or renames a unit file it did not just write in the same run.

# Acceptance Criteria

- A default install for a fresh team writes both the repair unit and the notifier unit; omitting either requires an explicit flag whose output names the omission and its consequence.
- A registry with a notifier unit but no repair unit is flagged by the check-only repair pass and by the host restore command, naming the missing unit and the exact install command; a fully paired registry reports no such finding.
- New installs generate the clear repair and notify names; the install output includes the migration note mapping each current generated name to its replacement, and no existing unit file is renamed, deleted, enabled, or started by the tool.
- Setup-generated units and services-generated units agree on the pair and the names (one shared rendering path or a test pinning both).
- Existing watch and watch-agents behaviour is unchanged for teams already running either unit under its current name.

# Notes

Filed on coordinator instruction after owner reply 244, alongside the restore script and scope work; implementation is sequenced after the registry-derived, slot-rebind, and resume-brief batch. Repair covers tab hygiene, queue drain, and health/repair; the notifier covers per-agent notices. Host unit renames and enablement stay with the host operator; this issue ships generation, detection, and the migration note only.
