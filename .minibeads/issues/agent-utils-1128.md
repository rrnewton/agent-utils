---
title: 'restore-deterministic-script: one deterministic restore command for every team on the host'
status: open
priority: 1
issue_type: feature
created_at: 2026-11-09T13:45:26.546506781+00:00
updated_at: 2026-11-09T13:45:32.652510911+00:00
---

# Description

Found during the 2026-11-09 host reboot after a btrfs volume went read-only. Restoring the host's agent teams required a hand-run entrypoint outside version control plus manual revive, slot rebind, and resume messages. The per-registry command is deterministic, but there is no single host command that waits for prerequisites, restores every team, and reports the outcome in one place.

Today the host entrypoint is a dotsynced script outside this repository. It refreshes the user certificate only when an agent forward is present and otherwise warns and continues, starts the terminal server with a short poll, then loops a hard-coded team list calling the per-registry restore. Its caller-visible behaviour is not deterministic: a missing certificate still attempts launches that then fail, a per-registry timeout can truncate a legitimate restore, exit codes collapse refused lanes into success, and a rerun after a failed boot reads as a fresh no-op. Both live registries share one default per-boot report file, so a rerun can read another registry's outcome.

Impact: the next reboot still needs a human or an agent to sequence certificate, server, per-team restores, slot rebinds, and resume notices by hand. The owner asked for one deterministic script after reboot instead of manual restore work.

# Design

Add one host command, restore-teams, that owns only host sequencing and reporting; per-registry revive stays in the existing per-registry restore.

Location: a new restore-teams subcommand beside the per-registry restore in the primary agentctl edition, with the mirror edition refusing it as primary-only like restore itself. The boot units call the same subcommand. No new top-level tool, no separate script inventory. A thin host shim may supply the host config path and the certificate fetch pre-step, but it holds no restore logic.

Configuration is config-driven, never hard-coded team or host names: an ordered list of registries with absolute tool paths (for example a teams JSON passed with --config, or repeated --registry). Each registry's own restore.json still names its certificate file and fetch command, its coordinator, and its units. The host command reads those; it does not embed a certificate path, a fetch command, a team layout, or a host map.

Contract: one command, idempotent. Exit 0 means every configured registry is back with no refusal. Exit 74 means the run finished with named refused lanes and a boot unit must not retry it as a temporary failure. Exit 75 means retry later (for example no certificate yet, server not up, or a registry incomplete for a temporary reason). Any other non-zero is a usage or permanent error. The aggregate is 75 if any registry is 75 or incomplete, else 74 if any registry refused a lane, else 0.

Sequence: bounded wait for the user certificate named by each registry (presence and validity as that registry defines it), bounded wait for the terminal server (start its unit once if configured, otherwise wait; never start a competing raw server), then restore each registry in config order with a timeout that exceeds the per-registry herdr wait plus per-lane startup delays. Print one per-agent table covering every registry (revived, promoted, skipped, excluded, refused with reason) from the per-registry reports, not from scraped logs. On an already-restored replay, label rows as replayed.

Reports: read the per-registry restore-reports location (the per-registry subdirectory under the registry state, moved there from the shared per-boot file). The shared default file must not be used for multi-registry decisions. The host command never calls the slot manager's adopt; slot rebind is the per-registry revive's transfer-owner step, surfaced as a table column.

# Acceptance Criteria

- One invocation on a host with two configured registries restores both, prints one table with a row per agent including refused lanes by name, and exits 0 when nothing was refused.
- When the user certificate is missing, the command waits bounded and then exits 75 naming the registry remedy, without launching any harness that needs the certificate.
- When one lane is unrestorable, the run completes the other lanes, names the refused lane in the table, and exits 74; a boot unit that treats 74 as success does not retry it.
- An immediate rerun in the same boot replays the recorded per-agent outcomes (labelled as replayed) and returns the same 0/74/75 class as the recorded run; it never reads as a fresh empty success.
- The boot service template installed for the host command calls the same subcommand with the same config shape, with a success status that includes 74 and a bounded retry only for 75.
- No hard-coded team, host, certificate-path, or fetch-command value is present in the tracked implementation; the client-names check stays green.

# Notes

Owner request 2026-11-09 after the btrfs reboot: the next reboot should be followed by a single deterministic script instead of manual restore work. Depends on the per-registry report location moving to a per-registry subdirectory (tracked under the boot-restore-retry issue remainder) and on the registry-derived, slot-rebind, resume-brief, and rerun-truth issues filed alongside it. Host file application (enabling units, supplying the teams config) stays with the host operator; this issue ships only the tracked command and its unit template. Proof is the reboot-free drill issue.
