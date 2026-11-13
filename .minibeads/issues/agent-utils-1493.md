---
title: 'services-turnover-unexpanded-specifiers: turnover does not expand systemd specifiers in ExecStart, so the registry and pin lookup miss'
status: closed
priority: 1
issue_type: bug
created_at: 2026-11-13T15:36:52.650550835+00:00
updated_at: 2026-11-13T17:32:21.515916064+00:00
closed_at: 2026-11-13T17:32:21.515915963+00:00
---

# Description

[watermelon-full-preview] Read-only `agentctl services turnover --report` on a host shows a unit whose `ExecStart` is `%h/.local/bin/agentctl chat run --state %h/.local/state/...` with no `--registry` listed with registry `%h/work/<project>/.agentctl`: the systemd specifier `%h` is not expanded when turnover derives the unit's registry. Its pin file under the real home `work/<project>/.agentctl/turnover-pins/<unit>.pin` is therefore never found, the unit reports an "unknown" commit, and the whole daily turnover exits 75, refreshing nothing on the host, two mornings running.

# Design

1. Expand systemd specifiers (`%h`, `%u`, `%U`, `%t`, `%S`, `%C`, `%E`, `%%`, at least those agentctl units use) when turnover/services parse a unit's `ExecStart`, `Environment` and drop-ins, for the registry, state dirs, binary path and pin lookup. Use the user manager's values, or `systemctl show`'s resolved `ExecStart` if available; prefer the resolved form over re-implementing.
2. Report clearly when a pin was looked for and where.
3. Open design point (awaiting coordinator OK, not landed unilaterally): today any unknown unit makes the whole run exit 75 and refresh nothing. Proposal: skip and alert the unknown unit, refresh the rest, exit 74/75 with the unknown named, since the owner wants upgrade-by-default.

# Acceptance Criteria

- A unit fixture with `%h` in `ExecStart` and no `--registry` resolves its registry and pin path to the real home; the pin is honoured.
- Rust (`rs/agentctl` services) and any Python equivalent stay in parity.
- The report states where a pin was looked for.
- No wall-clock waits in tests. Never run `services refresh` or non-report `turnover` against real units; tests use fixtures.

# Notes

Number allocated by the coordinator. Evidence is the read-only report described above.

## Resolution

Landed on `github/main` as `45a9c2315a9eefb1614376513e0815b5eaa7813b` ("agentctl: expand systemd specifiers in services turnover, skip unknown units"). Implemented on branch `mu-turnspec/1493`:

- `SpecifierContext` in `rs/agentctl/src/services/unit.rs` expands `%h`, `%u`, `%U`, `%t`, `%S`, `%C`, `%E` and `%%` (unknown specifiers left as written), with the user manager's values supplied by `Host::specifier_context` (`rs/agentctl/src/services/host.rs`, from `HOME`/`USER`/uid/`XDG_*`). `systemctl show` already prints a resolved `ExecStart`, but services derive paths from `systemctl cat`, so expansion reproduces the manager's resolution there.
- `read_service_any` (`discover.rs`) expands, for non-transient units, `ExecStart` words (including a `sh -c` script word), `Environment` values and `WorkingDirectory` — drop-ins are already merged by `UnitText::parse` — before deriving the registry, binary path, bridge state and state dirs. A unit naming its own `HOME` names its own `%h`. Transient units carry literal words and are not expanded. There is no Python equivalent of services turnover; the Rust surface is the only one.
- Turnover rows now carry `pin_checked`/`pin_path`, and the text report names where each pin was looked for (`turnover-pins/<unit>.pin`, then `turnover.pin`, or `global.pin`) and which was found.
- Item 3, as amended and approved by the coordinator for this landing: an unknown unit (no commit reported, incomparable commits, or an unreadable unit) is skipped and never touched, named in the report and in a per-unit `run-failed` notify with the reason and the remedy (pin it or move it under services); every other eligible unit refreshes as usual (bridges last, global last), `--report`/`--dry-run` show the same plan, and the run exits 75 if any unit was skipped as unknown (a rollback failure keeps 4), else the existing codes.
- Tests: `turnover_expands_percent_h_in_execstart_and_honours_the_pin_it_finds` (fixture with `%h` in `ExecStart`/`WorkingDirectory`, no `--registry`, registry and pin resolved to the real home, pin honoured, pin path named), `turnover_skips_an_unknown_unit_and_refreshes_the_rest` (unknown plus refreshable units: the rest refresh, the unknown is untouched and named in the notify, exit 75), and `expands_the_specifiers_agentctl_units_use`. Both turnover tests failed before the fix (registry literally `%h/work/team/.agentctl`; apply run exited 0) and pass after. No wall-clock waits; no real units were refreshed.
