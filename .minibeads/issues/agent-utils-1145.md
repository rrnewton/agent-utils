---
title: 'repair-notify-pair: install the repair unit beside the notifier with clear names'
status: open
priority: 1
issue_type: feature
created_at: 2026-11-09T16:52:51.923846039+00:00
updated_at: 2026-11-09T17:11:16.939921001+00:00
---

# Description

Owner request 2026-11-09 (reply 244), found while reviewing the unit sets after the reboot restore. The repair pass (tab hygiene, queue drain, health and repair of the terminal server, units, timers, slot images and this boot's restore) runs only when a team has a watch unit. The notifier pass (telling a manager or the coordinator when an agent is blocked, idle, stalled, or finished) is a different command with its own unit. On one host both registries have only the notifier, so their workspaces are never tidied, queued prompts are never drained by the repair pass, and nothing repairs a stopped server or timer there. Nothing in the default install path prevents that half-installed state, and nothing flags it afterwards.

The generated names make the gap easy to miss: the repair unit and the notifier units carry near-identical watch names, so an operator listing units cannot tell at a glance which behaviour each one runs, and a team can believe repair is covered when only notification is.

Impact: a team installed through the default path can run indefinitely with notification but no repair. The failures repair exists to fix then present as unrelated stalls: orphan shells accumulate, queues sit untyped, and a dead server or timer stays dead until a person intervenes.

Rescope (owner replies 256-257, 2026-11-09): repair and notification converge on one daemon with per-duty switches (Phase 1 switches, Phase 2 fold-in with a compatible alias), and the watch settings move from restore.json to REGISTRY/watch.json with a backward-compatible read, since the daemon reads them on every pass rather than once per boot. See Design.

# Design

Rescoped 2026-11-09 (owner reply 257): there is no good reason for repair and notification to be separate daemons beyond blast radius, so converge on ONE daemon with per-duty switches, in two phases. The slug and number stay; the pairing, detection, naming, and config-file parts of the original issue are kept and folded in below.

Phase 1 (small, first): the repair daemon gains per-duty switches in its config and as flags, each duty independently on or off, defaulting to today's behaviour when unset: notify, status_requests, tab_hygiene, unit_repair, herdr, slot_images, restore_check, queue_drain. A disabled duty is reported as skipped-by-config on every pass (including check-only), never as healthy and never repaired. The motivating case is a team whose bridge is owner-managed: with unit_repair off it can run hygiene and notification without the daemon ever restarting its bridge, while herdr, timers, and restore checks keep their own switches. Flags override config for one run; an unknown duty name in either place is refused at load.

Phase 2: fold the notifier duties (per-agent notices, blocked-prompt alerts, idle and stall classification, status-request pages) into the same daemon, so a host runs one unit per registry. The existing notifier command stays as a compatible alias or mode for installed units until they are migrated; its flags keep their meanings. The generated combined unit takes a name that states the single-daemon behaviour (for example an agent-scoped name per team). Service discovery (including the turnover flow, coordinated with its owner lane) must recognise all three unit kinds: the current repair unit, the current notifier unit, and the new combined unit.

Config file naming (owner reply 256, decided here): the repair daemon reads its hygiene and repair settings on every pass, so keeping them in a file named for boot restore misleads. Decision: watch settings move to a clearly named REGISTRY/watch.json (schema agentctl-watch/v1) holding the duties switches, herdr_unit, timers, bare_shell_grace_seconds, and close_bare_shells. Reads are backward compatible: watch.json wins when present; otherwise the same keys are read from restore.json exactly as today, with a warning naming watch.json as their home. New installs write watch.json and leave restore.json holding only restore keys; no existing file is rewritten automatically. Help text and the user guide state prominently which file each command reads on every pass versus once per boot. A key set in both files is refused with both locations named, never silently merged.

Kept from the original issue: the default install path (services snapshot flow and setup unit generation) installs the full duty set so a fresh team cannot end up with only half; the check-only pass and the host restore command flag a registry running only half the duties (by unit presence in Phase 1, by effective duties once switches exist), naming what is missing and the exact remedy; generated names state the behaviour; the install output carries the migration note (current names, replacement names, operator steps); live units are renamed by the host operator or coordinators, never by the tool, which never deletes or renames a unit file it did not just write.

# Acceptance Criteria

- Phase 1: with no duties configured, every pass behaves exactly as today; with unit_repair off (config or flag), a pass tidies tabs and drains queues but never resets, restarts, or starts any configured unit, and its health report lists unit_repair as skipped-by-config on every pass.
- Phase 1: each named duty can be switched off independently and the others still run; an unknown duty name in config or on the command line is refused before any pass runs, naming the duty.
- Config naming: a registry with watch settings only in restore.json keeps working unchanged (with the migration warning); the same settings in watch.json take effect identically; a key set in both files is refused naming both paths; new installs write watch.json.
- A registry running only half the duties is flagged by the check-only pass and by the host restore command, naming the missing duties and the exact remedy; a full-duty registry reports no such finding. A default install writes the full duty set; narrowing it requires an explicit switch whose output names the omission.
- Phase 2: one combined unit per registry performs repair and notification; an existing notifier unit invoking the alias or mode behaves as today until migrated; service discovery (including turnover) lists old repair, old notifier, and new combined units without double-counting a registry.
- New installs generate the clear single-daemon name; install output includes the migration note mapping each current generated name to its replacement, and no existing unit file is renamed, deleted, enabled, or started by the tool. Setup-generated and services-generated units agree (one shared rendering path or a test pinning both).

# Notes

Filed on coordinator instruction after owner reply 244, alongside the restore script and scope work; implementation is sequenced after the registry-derived, slot-rebind, and resume-brief batch. Repair covers tab hygiene, queue drain, and health/repair; the notifier covers per-agent notices. Host unit renames and enablement stay with the host operator; this issue ships generation, detection, and the migration note only.

Sequencing: Phase 1 is small enough to implement in a parallel worktree while earlier gates queue, landing immediately after the deterministic-script batch; Phase 2 follows the restore scope batch. Turnover service discovery overlaps the restore-core lane (old repair, old notifier, and new combined units); coordinate through the coordinator before editing shared discovery code.
