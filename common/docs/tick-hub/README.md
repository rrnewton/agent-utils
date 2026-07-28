# tick-hub

One scheduled **tick** — a single cron job, coordinator loop, or systemd timer — that funnels **many
recurring responsibilities**, each on its own cadence, and emits a stable, line-oriented
`HEALTH`/`ACTION`/`NOTE`/`ERROR` report for a coordinator or automation to dispatch.

The key idea: many systems can only run **one** recurring loop cheaply (a single `/loop`, one cron
line, one heartbeat). Rather than spin up N timers, tick-hub lets that one heartbeat carry every
recurring chore. On each tick it checks only the reminders that are **due**, runs their optional
checks, and prints exactly what needs attention. A reminder that is not due prints nothing; a slow
reminder (say a 6-hourly sync) appears only when its cadence has elapsed.

Everything domain-specific — which reminders exist, how often they run, what shell check gates them,
and what they emit — is **caller config** (JSON or YAML). The engine hardcodes no reminders, skills,
paths, or gates.

## What you get

- **A cadence hub.** Each reminder declares `cadence_secs` (`0` = every tick). tick-hub tracks each
  reminder's last-fired epoch in a tiny `key=last_fired_epoch` file so a slow reminder fires on
  schedule without its own timer — and a reminder that is not yet due emits nothing.
- **A machine-readable output contract.** Four line types, parsed by the leading token:
  `HEALTH:` (freshness of an artifact), `ACTION:` (one unit of work to dispatch, `skill` + fields +
  title), `NOTE:` (informational), `ERROR:` (an operational failure that needs attention).
- **Declarative reminders with shell gates.** A reminder can carry an optional `gate` — a shell
  command whose result decides whether it fires (`success` / `failure` / `nonempty` / `always`) — and
  can **capture** `key=value` lines from the gate's stdout to fill `{placeholders}` in the emitted
  line. So a reminder can report live values (a count, a SHA, a URL) with nothing hardcoded.
- **A typed per-host runtime state.** A small, strict ops-state file (`enabled`,
  `tick_frequency_min`, `label`, and a caller-defined `flags` map) gates reminders via
  `requires_flags` and drives an `actualize-tick-frequency` ACTION when the desired cadence differs
  from the running one.
- **Freshness health checks.** Point a check at a file glob with a staleness threshold; the tick
  reports `ok` / `stale` / `missing` so you can see when an upstream cron falls behind.

The engine is **pure and deterministic** given `now` (a parameter, so tests pin the clock). The two
side-effecting boundaries — running a gate command and measuring a file age — are the pluggable
`GateRunner` / `FileAgeProbe` protocols, with production implementations in `tick_hub.probes` and
fakes in the tests.

This is one tool from [`agent-utils`](https://github.com/rrnewton/agent-utils). It ships **Python
first** (mypy `--strict`, zero explicit `Any`); a Rust port + py↔rs differential is a planned
follow-up (see [Status & limitations](#status--limitations)).

## Install

```sh
pip install "git+https://github.com/rrnewton/agent-utils#subdirectory=py"
```

This installs the `tick-hub` console script and the importable `tick_hub` package. Requires Python
3.10+. For a self-contained tour any time:

```sh
tick-hub quickstart
```

## 60-second quickstart

Write a reminder set as `ops.yaml`. Each reminder has a `name`, a cadence, and an `emit` template;
optionally a shell `gate` and a list of `requires_flags`:

```yaml
reminders:
  - name: git_sync                 # a plain timed reminder
    cadence_secs: 21600            # ~6h; 0 = every tick
    emit: {kind: action, skill: git-sync, title: "fetch origin and reconcile"}
  - name: backlog                  # a GATED reminder that CAPTURES a live value
    cadence_secs: 0
    gate: {cmd: "echo count=42", when: always, capture: true}
    emit:
      kind: action
      skill: backlog-triage
      fields: {threshold: "20"}
      title: "backlog has {count} ready items (threshold {threshold})"
  - name: benchmark                # only fires when a state flag is truthy
    requires_flags: [benchmark_enabled]
    emit: {kind: action, skill: run-benchmark, title: "refresh benchmarks"}
health_checks:
  - name: db_backup
    glob: "/var/backups/db-*.sql"
    threshold_secs: 93600
    detail: "newest DB snapshot"
```

Run one tick (dry-run: prints the lines, persists no state):

```sh
$ tick-hub tick --config ops.yaml --state host.yaml --no-header
HEALTH: db_backup missing age_secs=NA threshold_secs=93600 detail="newest DB snapshot"
NOTE: ops-state enabled=true tick_frequency_min=30 label=my-host flags=benchmark_enabled=true
ACTION: git-sync title="fetch origin and reconcile"
ACTION: backlog-triage threshold=20 count=42 title="backlog has 42 ready items (threshold 20)"
ACTION: run-benchmark title="refresh benchmarks"
NOTE: emitted 3 instruction(s) this tick
```

Each `ACTION:` is one unit of work for a coordinator to dispatch to the named `skill`/handler; the
`HEALTH:` line is a freshness signal to investigate (not itself a work item). Add `--flush` to
persist the per-reminder last-fired epochs so cadenced reminders fire on schedule across ticks.

Ready-to-run examples live in
[`py/tick_hub/examples/`](https://github.com/rrnewton/agent-utils/tree/main/py/tick_hub/examples)
as a JSON/YAML twin (`tick-hub-ops.{yaml,json}`) plus a sample ops-state (`tick-hub-state.yaml`).

## The output contract

Every tick emits, in order: a `HEALTH:` line per health check, the ops-state's own lines (a summary
`NOTE:`, an optional `actualize-tick-frequency` ACTION, a disabled `NOTE:`), then one line per fired
reminder, then a trailing `NOTE:` with the count of instructions emitted.

```
HEALTH: <check> <ok|stale|missing> age_secs=<N|NA> threshold_secs=<N> detail="..."
ACTION: <skill> [key=value ...] title="..."
NOTE:   <free text>
ERROR:  <text>
```

Lines are independent — parse by the leading token. Field **values** are bare when safe and
double-quoted (with `\` and `"` escaped) when they contain whitespace, a quote, or are empty; the
`title` is always quoted. A caller dispatches each `ACTION` to the named skill; `HEALTH` `stale` /
`missing` means an upstream cron is behind (investigate, do not dispatch); `NOTE` is informational;
`ERROR` is an operational failure (e.g. a gate command that could not run).

## Reminders: when to check, whether to fire, what to emit

Each reminder is three independent decisions:

1. **When to check** — `cadence_secs`. `0` means every tick; otherwise the reminder is checked once
   at least that many seconds have elapsed since it last fired. The cadence governs how often the
   (possibly expensive) gate runs, not merely how often the line is printed.
2. **Whether to fire** — `requires_flags` and `gate`. `requires_flags` lists ops-state flags that
   must **all** be truthy, or the reminder is suppressed (and does *not* consume its cadence, so it
   fires promptly once the flag flips on). The optional `gate` is a shell command whose result
   decides firing:
   - `when: success` — fire on exit 0.
   - `when: failure` — fire on non-zero exit.
   - `when: nonempty` — fire when the command prints non-whitespace to stdout.
   - `when: always` — always fire (run the command only to **capture** values).
   With `capture: true`, the gate's stdout `key=value` lines are merged into the ACTION's fields and
   interpolated into `{placeholders}` in the title/text and field values.
3. **What to emit** — `emit`: `kind: action` (`skill`, `fields`, `title`) or `kind: note` (`title`).

A gate that cannot run at all (missing binary, timeout) produces an `ERROR:` line and does **not**
stamp the reminder's last-fired epoch, so it is retried next tick (No Silent Failure).

## The ops-state (per-host runtime state)

A small, strict, typed YAML file, read at the start of each tick (`--state FILE`):

```yaml
enabled: true            # master switch; false => summary + health checks only, no reminders
tick_frequency_min: 30   # desired cadence; --current-tick-min N emits an actualize ACTION if N differs
label: my-host           # optional identity shown in the state summary line
flags:                   # caller-defined toggles read by requires_flags (bool / int / string)
  benchmark_enabled: true
  ops_in_charge: true
```

Unknown top-level keys, missing required fields, and wrong types are hard errors (the CLI exits 2
with a clear message). Flags are the extension point: name them whatever your reminders gate on. When
`--state` is omitted or the file is missing, a sensible enabled default is used (with a loud stderr
notice).

## CLI reference

```
tick-hub <command> [options]
```

| Command      | What it does                                                              |
| ------------ | ------------------------------------------------------------------------- |
| `tick`       | Run one tick: emit the HEALTH/ACTION/NOTE/ERROR lines. Dry-run by default (no state write); `--flush` persists the fired-state. |
| `state`      | Validate an ops-state file and print its own state-machine lines.         |
| `list`       | List the reminders (cadence, gate, flags) and health checks.             |
| `json`       | Re-emit the config as canonical, fully-defaulted JSON.                    |
| `yaml`       | Re-emit the config as YAML.                                              |
| `quickstart` | Print a self-contained getting-started guide (no `--config` needed).     |
| `--userguide`| Print this full user guide (the complete reference), embedded in the package so it works after `pip install`. |

Global: `--version`, `--userguide`, `-h/--help`. Running with no command prints help and exits `0`.

### `tick` flags

| Flag                 | Meaning                                                                             |
| -------------------- | ---------------------------------------------------------------------------------- |
| `--config FILE`      | Reminder-set config (`.yaml`/`.yml` → YAML, else JSON). Required.                  |
| `--state FILE`       | Per-host ops-state YAML (optional; a sensible enabled default is used when omitted, with a notice). |
| `--fired-state FILE` | Per-reminder last-fired-epoch file (default `./.tick-hub/state`, or `$TICK_HUB_STATE`). |
| `--now EPOCH`        | Override the clock with an explicit epoch (seconds) for a deterministic tick.       |
| `--current-tick-min N` | The actually-running tick cadence (minutes); emit `actualize-tick-frequency` if it differs from the ops-state's `tick_frequency_min`. |
| `--flush`            | Persist the advanced fired-state (default is a dry-run that mutates nothing).      |
| `--no-header`        | Suppress the explanatory stderr banner (pure machine parsing).                    |

## Python API

The same engine is available as a library:

```python
from tick_hub import TickConfig, Reminder, Emit, EmitKind, Gate, GateWhen, HealthCheck
from tick_hub import OpsState, run_tick
from tick_hub.probes import SubprocessGateRunner, GlobFileAgeProbe

cfg = TickConfig(
    reminders=(
        Reminder("git_sync", Emit(EmitKind.ACTION, "fetch origin", skill="git-sync"),
                 cadence_secs=21600),
        Reminder("backlog", Emit(EmitKind.ACTION, "{count} ready", skill="triage"),
                 gate=Gate("echo count=42", when=GateWhen.ALWAYS, capture=True)),
    ),
    health_checks=(HealthCheck("db_backup", "/var/backups/db-*.sql", 93600, "newest snapshot"),),
)
result = run_tick(cfg, OpsState.default(), now=0, fired={},
                  gate_runner=SubprocessGateRunner(), age_probe=GlobFileAgeProbe())
for line in result.lines:
    print(line)
```

`run_tick` returns a `TickResult` (`.lines`, `.fired` — the advanced last-fired map — and
`.actions_emitted`). Inject a fake `GateRunner` / `FileAgeProbe` to test tick behavior
deterministically. See [`USER_GUIDE.md`](USER_GUIDE.md) for the full schema and API.

## Exit codes

| Code | Meaning                                            |
| ---- | -------------------------------------------------- |
| `0`  | The tick ran (whether or not any ACTION was emitted). |
| `2`  | Bad usage, or a missing / malformed config or ops-state file. |

## Status & limitations

- **Python CLI + API: ready.** `tick`, `state`, `list`, `json`, `yaml`, and `quickstart` all work;
  mypy `--strict` passes with zero explicit `Any`. The cadence/due-logic, gate evaluation (with
  capture + interpolation), flag gating, health checks, and the ops-state state machine are complete
  and unit-tested (including exact clock boundaries and the output-line bytes).
- **YAML is isomorphic to the JSON schema and Rust-port-ready.** Both config surfaces funnel through
  one strict typed narrowing, and plain YAML scalars resolve with YAML-1.2 core-schema rules (the
  "Norway problem" tokens stay strings), matching the maintained Rust YAML parser a port would use.
- **Follow-ups (clearly scoped):** (1) a **Rust port + `cross/` differential** that feeds identical
  configs/states to both builds and asserts byte-identical `HEALTH`/`ACTION`/`NOTE`/`ERROR` output —
  exactly the parity guarantee the sibling `safe-ci-dag-runner` already ships; and (2) switching a
  real caller — DeepScry's hand-written ops poller — to consume this generic engine, moving its
  bespoke reminders/health-checks into a tick-hub config.

## See also

- [`USER_GUIDE.md`](USER_GUIDE.md) — the full config + ops-state schema, the gate/capture model, the
  in-depth Python API, and troubleshooting.
- [`examples/`](https://github.com/rrnewton/agent-utils/tree/main/examples) — a runnable JSON/YAML
  config twin plus a sample ops-state.
- `tick-hub quickstart` — the same tour from the command line.

## License

MIT.
