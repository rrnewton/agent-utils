---
name: tick-hub
description: One scheduled tick that funnels many recurring responsibilities, each on its own cadence, into machine-readable HEALTH/ACTION/NOTE/ERROR lines for a coordinator or automation to dispatch. Use when consolidating N separate timers/cron reminders into a single heartbeat, or wiring cadenced gates and freshness health checks that emit parseable output.
---

# tick-hub

A single scheduled "tick" (a cron job, coordinator loop, or systemd timer) checks every DUE
reminder — plain timed, flag-gated, or shell-gated with captured values — plus file-freshness
health checks, and emits a stable, line-oriented `HEALTH:` / `ACTION:` / `NOTE:` / `ERROR:` report.
One loop carries every recurring responsibility instead of N separate timers. Reminders, gates, and
health checks are all caller config; the engine is pure and deterministic given `now`. Each
reminder's records open with a `[tick-hub: NAME · SCHEDULE]` line, so a record forwarded to an
agent says which reminder sent it.

A config may declare a whole-tick `deadline_secs` budget (accounted in each reminder's
`max_duration_secs` bound) and typed `after` ordering edges between reminders, optionally
requiring the prerequisite to succeed. A reminder that cannot start emits a `SKIPPED:` line
naming it and keeps its cadence for the next tick; see `tick-hub --userguide` for the contract.

The CLI is the source of truth for usage — do not rely on this file for details. Run:

- `tick-hub quickstart` — self-contained getting-started tour (no repo needed).
- `tick-hub --help` — commands and flags.
- `tick-hub --userguide` — the full user guide (complete reference).
