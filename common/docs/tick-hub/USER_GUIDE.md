# tick-hub — User Guide

This guide goes deeper than the [README](README.md): the mental model, the complete config and
ops-state schemas, the gate/capture model, the full Python API, and troubleshooting. To just get
running, start with the README's 60-second quickstart or `tick-hub quickstart`.

## Contents

- [The model: one tick, many cadenced reminders](#the-model-one-tick-many-cadenced-reminders)
- [The config schema (reminders + health checks)](#the-config-schema-reminders--health-checks)
- [The ops-state schema (per-host runtime state)](#the-ops-state-schema-per-host-runtime-state)
- [Gates and value capture](#gates-and-value-capture)
- [Cadence and the fired-state file](#cadence-and-the-fired-state-file)
- [The output contract](#the-output-contract)
- [YAML: an isomorphic, literate alternative to JSON](#yaml-an-isomorphic-literate-alternative-to-json)
- [The Python API](#the-python-api)
- [Wiring tick-hub into a scheduler](#wiring-tick-hub-into-a-scheduler)
- [Troubleshooting](#troubleshooting)
- [Relationship to a hand-written poller (what was generalized)](#relationship-to-a-hand-written-poller-what-was-generalized)

## The model: one tick, many cadenced reminders

A **tick** is one heartbeat of some outer scheduler you already have: a cron line, a coordinator
`/loop`, a systemd timer. Many such schedulers can only cheaply run **one** recurring thing. tick-hub
makes that one thing carry *all* of your recurring responsibilities.

On each tick, tick-hub:

1. evaluates every **health check** (a freshness probe over a file glob),
2. emits the **ops-state** machine's own lines (a summary, a tick-frequency actualization ACTION if
   the desired cadence differs from the running one, and a disabled note when off),
3. checks each **reminder** that is *due* (its cadence has elapsed) and whose `requires_flags` are
   all truthy: it runs the reminder's optional shell **gate**, records that the check ran, and emits
   the reminder's `ACTION`/`NOTE` when the gate fires, and
4. prints a trailing `NOTE` with the count of instructions emitted.

The result is a stable, line-oriented report a coordinator (human or automation) consumes: dispatch
each `ACTION`, investigate each non-`ok` `HEALTH`, read the `NOTE`s, escalate any `ERROR`.

## The config schema (reminders + health checks)

A config document (JSON or YAML) has three top-level fields, all optional:

| Field           | Type               | Default | Meaning                                        |
| --------------- | ------------------ | ------- | ---------------------------------------------- |
| `description`   | string             | `""`    | Free-form docs for the whole config (never affects behavior). |
| `reminders`     | list of reminder   | `[]`    | The recurring responsibilities.                |
| `health_checks` | list of health check | `[]`  | Freshness probes.                              |

A **reminder**:

| Field            | Type            | Default    | Meaning                                                    |
| ---------------- | --------------- | ---------- | --------------------------------------------------------- |
| `name`           | string          | (required) | Unique key; also the fired-state key.                     |
| `emit`           | emit object     | (required) | What to produce when it fires (see below).                |
| `cadence_secs`   | integer         | `0`        | How often to CHECK; `0` = every tick.                     |
| `requires_flags` | list of string  | `[]`       | Ops-state flags that must ALL be truthy, or it is suppressed. |
| `gate`           | gate object     | `null`     | Optional shell check deciding whether to fire.            |

An **emit**:

| Field    | Type              | Default    | Meaning                                                     |
| -------- | ----------------- | ---------- | ---------------------------------------------------------- |
| `kind`   | `action` / `note` | `action`   | An `ACTION:` line (dispatchable) or a `NOTE:` line.        |
| `title`  | string            | `""`       | The ACTION title (quoted) or the NOTE text. Supports `{placeholder}` interpolation. |
| `skill`  | string            | `""`       | The ACTION handler name (required for `kind: action`).     |
| `fields` | map string→string | `{}`       | ACTION `key=value` fields (values support `{placeholder}` interpolation). |

A **gate** (see [Gates and value capture](#gates-and-value-capture)):

| Field     | Type                                        | Default   | Meaning                                    |
| --------- | ------------------------------------------- | --------- | ------------------------------------------ |
| `cmd`     | string                                      | (required) | Shell command, run via `bash -c`.         |
| `when`    | `success` / `failure` / `nonempty` / `always` | `success` | The fire condition.                       |
| `capture` | boolean                                     | `false`   | Parse stdout `key=value` lines for interpolation + fields. |

A **health check**:

| Field            | Type    | Default    | Meaning                                                |
| ---------------- | ------- | ---------- | ----------------------------------------------------- |
| `name`           | string  | (required) | The check name in the HEALTH line.                    |
| `glob`           | string  | (required) | File glob; the NEWEST match's mtime is measured.      |
| `threshold_secs` | integer | `0`        | `ok` at/under this age, `stale` above, `missing` if no match. |
| `detail`         | string  | `""`       | Human detail shown in the HEALTH line.                |

Parsing is **strict**: unknown/missing/wrong-typed fields raise an error (the CLI exits 2 with a
clear message). Integers are bounded to signed 64-bit so a config the Python build accepts is also
readable by the planned Rust build.

## The ops-state schema (per-host runtime state)

A separate, small YAML file (`--state FILE`) carries per-host runtime state — the generic analog of a
per-machine `.ops-state.yaml`. It is typically per-host and gitignored.

| Field                | Type              | Default | Meaning                                                 |
| -------------------- | ----------------- | ------- | ------------------------------------------------------- |
| `enabled`            | boolean           | (required) | Master switch. `false` ⇒ the tick emits its summary + health checks but fires no reminders. |
| `tick_frequency_min` | integer > 0       | `30`    | Desired tick cadence. With `--current-tick-min N`, a mismatch emits an `actualize-tick-frequency` ACTION. |
| `label`              | string / null     | `null`  | Optional identity shown in the state summary line.      |
| `flags`              | map string→(bool/int/string) | `{}` | Caller-defined toggles read by a reminder's `requires_flags`. |

Unknown top-level keys, missing required fields, and wrong types are hard errors. **Flags are the
extension point:** rather than the engine knowing about "am I the ops driver?" or "is benchmarking
on?", you put those as flags and gate the relevant reminders with `requires_flags`. A flag is
"truthy" when it is `true`, a non-zero int, or a non-empty string.

When `--state` is omitted, or points at a missing file, tick-hub uses a sensible enabled default and
prints a loud stderr notice; a *malformed* explicit state file is a hard error (exit 2).

## Gates and value capture

A gate is how a reminder asks "should I fire *right now*?" beyond just "is my cadence up?". The gate
`cmd` runs via `bash -c` (with a timeout) and `when` selects the fire condition:

- `success` — the command exited 0 (e.g. `[ -f /flag ]`).
- `failure` — the command exited non-zero (e.g. a health probe that returns non-zero when unhealthy).
- `nonempty` — the command printed non-whitespace to stdout (e.g. `gh pr list --json number -q '.[]'`).
- `always` — always fire; used purely to **capture** values.

With `capture: true`, the gate's stdout is scanned for `key=value` lines (blank and `#` lines
ignored). Those captured values are:

- **merged into the ACTION's `fields`** (captured wins over a static field of the same name), and
- **interpolated into `{placeholders}`** in the `title` and in each static field value.

So a reminder can carry live data with nothing hardcoded:

```yaml
- name: backlog
  gate: {cmd: "printf 'count=%s\\n' \"$(my-ready-count)\"", when: always, capture: true}
  emit:
    kind: action
    skill: backlog-triage
    fields: {threshold: "20"}
    title: "backlog has {count} ready items (threshold {threshold})"
# -> ACTION: backlog-triage threshold=20 count=42 title="backlog has 42 ready items (threshold 20)"
```

If a gate command **cannot run** (missing binary, timeout), the tick emits an `ERROR:` line and does
**not** stamp the reminder's fired epoch, so it is retried next tick — never silently skipped.

## Cadence and the fired-state file

`cadence_secs` is how often a reminder is **checked** (its gate run), not merely how often the line
prints. `0` means every tick; otherwise the reminder is due when at least `cadence_secs` have elapsed
since it last fired.

tick-hub records each reminder's last-fired epoch in a tiny `key=last_fired_epoch` text file:

```
# tick-hub fired-state — key=last_fired_epoch (managed by tick-hub)
backlog=1718600000
git_sync=1718580000
```

Resolution order for its location: `--fired-state DIR/FILE` → `$TICK_HUB_STATE` → the repo-local
default `./.tick-hub/state` (created on demand). It is written **only on `--flush`**; the default
dry-run mutates nothing, so you can preview a tick freely. A reminder absent from the file has never
fired and is due. Important semantics:

- A reminder whose check **runs** (it is due and its flags are satisfied) is stamped with `now`,
  whether or not its gate fired — so the cadence measures *check* frequency.
- A reminder **suppressed by a flag** is *not* stamped, so it fires promptly the tick after the flag
  turns on.
- A reminder whose gate **could not run** is *not* stamped (retried next tick).

Pass `--now EPOCH` to pin the clock, which (with `--flush`) makes a tick fully reproducible.

## The output contract

```
HEALTH: <check> <ok|stale|missing> age_secs=<N|NA> threshold_secs=<N> detail="..."
ACTION: <skill> [key=value ...] title="..."
NOTE:   <free text>
ERROR:  <text>
```

Ordering per tick: all `HEALTH` lines, then the ops-state lines (summary `NOTE`, optional
`actualize-tick-frequency` ACTION, disabled `NOTE`), then one line per fired reminder, then a
trailing `NOTE: emitted N instruction(s) this tick`. Lines are independent — parse by the leading
token. Field values are bare when safe and double-quoted (with `\` and `"` escaped) when they contain
whitespace, a quote, or are empty; the `title` is always quoted. (`[...]`-style progress and the
banner go to **stderr**, so stdout stays pure machine-readable; add `--no-header` to drop the banner.)

## YAML: an isomorphic, literate alternative to JSON

A config can be JSON or YAML — two spellings of the *same* schema (same fields, defaults, strict
validation, model). `--config FILE` auto-detects by extension (`.yaml`/`.yml` → YAML, else JSON). The
`json` and `yaml` subcommands re-emit a loaded config, so you can convert between them.

YAML additionally allows comments and multi-line block scalars, so a reminder set documents itself
inline. One gotcha to know: plain scalars resolve with **YAML-1.2 core-schema** rules (matching the
maintained Rust YAML parser a future port uses), *not* PyYAML's YAML-1.1 defaults. That means the
"Norway problem" tokens `no`/`yes`/`on`/`off` stay **strings**, not booleans — so `title: no` is the
string `"no"`. A boolean field still needs an explicit `true`/`false`. Loading is designed to be
byte-isomorphic across the Python and (planned) Rust builds.

## The Python API

```python
from tick_hub import (
    TickConfig, Reminder, Emit, EmitKind, Gate, GateWhen, HealthCheck,
    OpsState, run_tick,
)
from tick_hub.probes import SubprocessGateRunner, GlobFileAgeProbe

cfg = TickConfig(
    reminders=(
        Reminder(
            "ci_health",
            Emit(EmitKind.ACTION, title="CI on {branch} is red", skill="ci-health-red",
                 fields={"branch": "main"}),
            requires_flags=("ops_in_charge",),
            gate=Gate("gh-ci-status-check", when=GateWhen.FAILURE),
        ),
    ),
    health_checks=(HealthCheck("db_backup", "/var/backups/db-*.sql", 93600, "newest snapshot"),),
)

state = OpsState(enabled=True, tick_frequency_min=30, flags={"ops_in_charge": True})
result = run_tick(cfg, state, now=0, fired={},
                  gate_runner=SubprocessGateRunner(), age_probe=GlobFileAgeProbe(),
                  current_tick_min=30)
for line in result.lines:
    print(line)
print(result.actions_emitted, dict(result.fired))
```

`run_tick` returns a `TickResult` with `.lines` (the emitted report), `.fired` (the advanced
last-fired map to persist), and `.actions_emitted`. The two side effects are injected:

- `GateRunner.run(cmd) -> GateResult(returncode, stdout, ok, error)` — swap in a fake for
  deterministic tests (return canned results per command).
- `FileAgeProbe.newest_age_secs(glob, now) -> int | None` — swap in a fake returning canned ages.

Config and ops-state (de)serialization live in `tick_hub.io` (`config_from_json/yaml`,
`config_to_json/yaml`) and `tick_hub.state` (`OpsState.load` / `from_yaml` / `to_yaml`); the pure
line formatters are in `tick_hub.emit`.

## Wiring tick-hub into a scheduler

tick-hub does not schedule itself — it is invoked once per tick by *your* scheduler. Typical wiring:

- **cron:** a single line at your tick cadence running
  `tick-hub tick --config /etc/ops/ops.yaml --state /etc/ops/host.yaml --flush --no-header` and
  piping stdout to whatever consumes ACTIONs.
- **a coordinator loop / agent heartbeat:** call `tick --flush` each heartbeat, parse the lines, and
  dispatch each `ACTION` to the named handler (subject to your own concurrency limits).
- **systemd timer:** a `.timer` at the tick cadence driving a `.service` that runs the same command.

Keep one tick cadence as the single knob; set the ops-state's `tick_frequency_min` to your intended
cadence and pass `--current-tick-min` so a drift between the two is surfaced as an
`actualize-tick-frequency` ACTION.

## Troubleshooting

- **A reminder never fires.** Check (a) its cadence hasn't already been stamped in the fired-state
  file for this window; (b) all its `requires_flags` are truthy in the ops-state; (c) its gate's
  `when` matches the command's actual exit/stdout. Run `tick-hub list --config ops.yaml` to see the
  cadence/gate/flags at a glance.
- **A `{placeholder}` shows up literally.** The name isn't in the merged fields — it must be either a
  static `fields` key or a captured `key=value` from a `capture: true` gate.
- **`ERROR: reminder X: gate command could not run`.** The gate binary is missing from `PATH` or it
  timed out. tick-hub deliberately surfaces this instead of skipping silently.
- **A YAML value became the wrong type.** Remember plain `no`/`yes`/`on`/`off` are strings here;
  quote a value you mean as a string, and use explicit `true`/`false` for booleans.
- **The tick wrote nothing to the fired-state file.** The default is a dry-run — add `--flush`.

## Relationship to a hand-written poller (what was generalized)

tick-hub is the generic engine factored out of DeepScry's hand-written ops poller (a single Python
script with a wall of `do_<name>` functions plus a shell cadence-config file). The mapping:

| Hand-written poller                                   | tick-hub                                        |
| ----------------------------------------------------- | ----------------------------------------------- |
| a `do_<name>` function per responsibility             | a `Reminder` (data), gate = its check, emit = its output |
| per-responsibility cadence constants in a shell config | each reminder's `cadence_secs`                  |
| the `key=last_fired_epoch` state dotfile              | the fired-state file (unchanged format)         |
| `is_due(name, cadence, now, last)`                    | the pure cadence engine (`now` is a parameter)  |
| the `HEALTH:`/`ACTION:`/`NOTE:`/`ERROR:` emitter       | `tick_hub.emit` (same contract)                 |
| the newest-file-age health check                      | a `HealthCheck` + `FileAgeProbe`                |
| the typed per-host `.ops-state.yaml` reader           | the `OpsState` model + `requires_flags` gating  |
| `ops_in_charge` / `benchmark_enabled` / … toggles     | caller-defined `flags`                          |

Everything project-specific (which reminders, which skills, which paths, which gates) becomes caller
config; the engine keeps none of it. The two planned follow-ups are a Rust port with a py↔rs
differential (the parity guarantee the sibling `safe-ci-dag-runner` already ships) and switching the
DeepScry poller itself to consume this engine.
