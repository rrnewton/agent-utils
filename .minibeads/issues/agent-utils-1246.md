---
title: 'gate-success-rate: a daily gate success-rate report the owner can see'
status: closed
priority: 0
issue_type: feature
assignee: mu-gatestats
created_at: 2026-11-10T20:26:59+00:00
updated_at: 2026-11-11T11:07:51+00:00
closed_at: 2026-11-11T11:07:51+00:00
---

# Description

[watermelon-full-preview] Owner request (2026-11-10, reply 344): "we should monitor our success rates." Today gate outcomes live in two read-only sources no one summarises per day: the admission history (wait/run per ticket, class, priority) and the durable gate ledger (head_sha, preflight, kind, verdict, failed steps). The 2026-11-09 exclusive-bottleneck measurement was a one-off hand-built script; there is no repeatable daily view of whether gates are getting better or worse.

# Design

- A small standalone script `scripts/gate_stats.py` (not a gate.py subcommand: gate.py is admission-critical and its existing `stats` is a coarse all-time ledger summary). It reads only the two sources above, plus nothing else by default.
- CLI: `--day YYYY-MM-DD` (required, America/New_York day by queued time), `--json` or `--markdown` output, `--history`/`--ledger` path overrides for tests and other hosts, and `--output` to write to a file instead of stdout. Every argument is described in `--help` with defaults.
- Metrics per ET day, split by tier (owner when priority is owner, else normal) and class (shared, exclusive): first-try pass rate per head (the first full gate for a head_sha in the whole ledger, green on that first try), flake rate (red steps that later pass on a solo rerun of the same head, per step name, with the top 10 flaky steps), red-gate rate, red-batch rate (column ready, null until batching records exist), wait p50/p90 and run p50/p90, gates per hour, and exclusive host-hours.
- Unit tests on fixture records in the repository's normal test layout, covering the pass/flake/red calculations, tier and class splits, percentile and empty-day behaviour, and the batch column staying empty.
- A daily markdown page per day under a gate-stats subdirectory of the harness documentation, backfilled for 2026-11-08, 2026-11-09 and 2026-11-10, with one changelog bullet for the series; a host timer then writes the previous day's page each morning. The harness-side page and timer are delivered with the tool and named in the closing note.

# Acceptance Criteria

- `scripts/gate_stats.py --day 2026-11-09 --markdown` and `--json` produce the metrics above from the live read-only sources without modifying them, and `--help` describes every argument.
- Fixture-record unit tests pass, including a flaky step (red then solo-rerun pass on the same head), a first-try failure that later passes (not counted as first-try green), owner/normal and shared/exclusive splits, and an empty day.
- The three backfilled daily pages exist in the harness documentation with a changelog bullet for the series, and the timer script and unit for the morning run are present with their unit name reported.
- The tool lands on a normal full gate.

# Notes

Filed by mu-gatestats at the lead coordinator's request (owner reply 344, 2026-11-10). Prior method: the 2026-11-09 exclusive-bottleneck measurement in the harness documentation and its scratch script. Slot gatestats, branch mu-gatestats/gate-stats.

[watermelon-full-preview] Closed by mu-gatestats 2026-11-11. Tool landed 02664209 on GitHub main (`scripts/gate_stats.py`, fixture tests, validation classification; strict mypy clean, 4/4 fixture tests). Full gate on the rebased tip ran 243 passed / 2 failed, both untouched code: `python.repository-infrastructure.test` failed only on `test_interleave_puts_oldest_non_owner_next_after_two_owners` (untouched environmental red while the coordinator's temporary gate-interleave host file with after_owners = 99 is in place; fails on clean main too), and `python.dagrun.shard-1` failed on `test_proccpu.py::test_new_owned_reader_evicts_expired_compatibility_registration`, which passed on one solo rerun (501 passed). Local path-selected validate beforehand: 241 passed with the same interleave red plus three further untouched flakes, each solo-passed. Harness side (separate repository, main ce0ad7e): backfilled daily pages for 2026-11-08/09/10 under a gate-stats subdirectory with one changelog bullet, and a morning timer `agent-utils-gate-stats-daily.timer` (about 06:00 ET) whose script prefers the landed primary-checkout tool and fell back to the slot copy before landing.
