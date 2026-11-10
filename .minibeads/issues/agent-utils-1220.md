---
title: 'services-turnover-timer-gaps: close the gaps from the first live turnover before the 05:30 ET timer'
status: open
priority: 1
issue_type: task
created_at: 2026-11-10T15:41:18+00:00
updated_at: 2026-11-10T15:41:18+00:00
---

# Description

[watermelon-full-preview] Follow-up from the first live `agentctl services turnover` run on 2026-11-10 (candidate cca10042, #1179 services-turnover). Every refreshed unit on the host moved and passed its health check, both boot-restore units were updated without being started, and the global binary moved last. The daily 05:30 timer install waits until the timer-path items below land. One issue covers all of these gaps so they are fixed together.

Timer path, blocking a scheduled turnover:

- A timezone cannot be expressed. `install-units --scheduled` cannot render `*-*-* 05:30:00 America/New_York`: the calendar validator in `rs/agentctl/src/restore.rs` allows only letters, digits, space and a few punctuation marks, so the underscore in a timezone name is refused. A daily run must fire at 05:30 in that zone year-round, not at host-local time.
- The scheduled template is a bare oneshot. The service template rendered by `install-units --scheduled` has no start timeout, no PATH or HOME environment, and no ordering after the job that fast-forwards the local mirror to main. A cold snapshot build plus per-unit health windows takes many minutes, and a notify send needs the terminal and agent binaries on PATH.
- Exit and alert semantics are undefined. A report exits 75 whenever an unknown row exists even when nothing is behind, an apply can succeed while report-only rows stay unknown, and the scheduled service has no defined failure notification to the coordinator.
- A timer has no notify identity. A manual turnover inherits a caller identity for its notify sends; a timer service has none in its environment. Its notify command also needs a real prompt, since a bare send with no text or file sends nothing useful.

Discovery and reporting gaps found live:

- Discovery misses bridges started as `chat run --state DIR`. A bridge is classified only when that command carries `--bridge-state`. Units of the other shape exist on the host (inactive and disabled in this run, so nothing was left behind this time) and are invisible to turnover.
- Other-daemon rows report unknown forever. Units running non-agentctl binaries of this codebase are report-only by design, since no snapshot carries them, but because those binaries report no commit they take the unknown decision, which forces every report to exit 75 even when every agentctl service is current.
- Pin directories are scanned as agents. An empty pin directory created in a registry root was read by the watcher as an agent directory and alerted as unreachable. Pins created for a staged run also leave directories behind after the pin files are removed.
- Inline replies emitted while a bridge is down are dropped. During the first live bridge restart, an inline reply marker emitted by the agent behind that bridge was silently dropped, not queued, with no error. Inbound messages all arrived, and a later reply sent from a file posted fine.
- The global binary backup message is wrong in the symlink case. The backup file is written only when the global binary is not a symlink, but the result always names that backup path. Where the global binary is a symlink, rollback is repointing it at its previous target.

# Design

Timer path:

- Allow a timezone in the scheduled calendar validation, and any other valid calendar syntax the template can render safely, with tests for the exact 05:30 zone form.
- Extend the scheduled service template, or the scheduled spec entry, so a turnover entry can declare a start timeout long enough for a cold build and health windows, the environment it needs, and ordering after the mirror job. Render those into the written units.
- Define the mapping from turnover exit codes (0, 1, 4, 74, 75) to scheduled-service success or failure, and send a coordinator alert on the failures, through the same notify mechanism the manual run uses.
- Give the timer a designed notify identity (a named timer identity, or queue-only delivery it can prove), and make the timer use a notify wrapper that formats the phase, registry, unit, commits, and verdict environment into a file-based send.

Discovery and reporting:

- Classify the `chat run --state DIR` shape as a bridge, or document and enforce a migration of such units to the `--bridge-state` form, so an active bridge of that shape is never silently left behind.
- Give non-agentctl daemon rows a report-only decision when no snapshot can carry their binary, regardless of whether the binary reports a commit, or teach those binaries to report one. A report where every agentctl service is current should be able to exit 0 or 74.
- Make the registry agent scan skip the pin directory and other non-agent entries. Create the pin directory with private permissions if it stays in the registry, and remove it when its last pin file is removed.
- On bridge restart, pick up inline replies the target emitted while the bridge was down (scan the target output the bridge already reads, for reply blocks with no sent record), or, until that works, have turnover and the bridge warn agents behind a bridge before it restarts to use file-based replies during the window. Never drop such a reply silently: queue it, post it, or report it by name.
- Report the real rollback for the global binary: name the backup file when one is written, and name the previous symlink target when the binary is a symlink.

# Acceptance Criteria

- A scheduled spec entry rendering a 05:30 daily schedule in a named timezone passes validation and renders a timer that fires at that zone time, proven by a rendered-unit test.
- A rendered scheduled turnover service carries a start timeout that covers a cold snapshot build and the health windows, the environment its notify send needs, and ordering after the mirror job.
- Each turnover exit code has a defined scheduled-service outcome, and a failing outcome produces a coordinator alert that is observed in a test.
- A timer-driven run sends its phase notifications under its own designed identity, with the commits and verdict in the message.
- A unit running the `chat run --state DIR` shape is listed by turnover discovery with a bridge decision, or such a unit is refused with a named migration remedy.
- A report where every agentctl service is current and only other-daemon rows have no commit exits 0 or 74, and those rows read report-only.
- A registry containing a pin directory produces no watcher alert about it, and a staged run leaves no empty pin directory behind.
- A reply marker emitted while its bridge is down is posted after the restart, or is named as undelivered to the coordinator; in no case is it silently dropped. Inbound messages delivered during the same window still arrive.
- Moving a symlinked global binary reports a rollback that exists: the previous symlink target, named in the output.

# Notes

Live run evidence, 2026-11-10: the host report at the pinned candidate listed 12 services, 9 to refresh. The two registries on the host each moved their watch, watch-agents, and restore units (restores updated-inactive, not started), both chat bridges moved last in their scope and passed chat health, the global binary moved last and reported the candidate commit, and a new post through the restarted bridge was confirmed. The only pins left after that run are for an inactive legacy bridge, to be migrated by hand under services before unpinning.

The agent scan alert came from the current watcher on the candidate build reading a registry root, so the scan fix belongs with the watcher or the registry reader it uses, not only with pin creation. The dropped inline reply was reported by the team behind the restarted bridge; its number in their own tracking is theirs to cite, and this issue deliberately keeps host and team names out.
