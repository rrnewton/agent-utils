---
title: 'systemd-run-expands-environment-in-args: pass --expand-environment=no wherever a launcher hands systemd-run text it did not write'
status: open
priority: 1
issue_type: bug
assignee: cl-sysdexpand
labels:
- dagrun
- herdr-run
- wrkslots
- gate
created_at: 2026-11-26T21:31:00+00:00
updated_at: 2026-11-26T21:31:00+00:00
---

# Description

A trusted consumer reports, and it reproduces on a host with systemd
262, that `systemd-run --scope` now expands `${...}` in the command's
arguments by default, from the CALLER's environment:

    PROBEVAR=expanded systemd-run --user --scope --quiet \
        /bin/echo 'A:${PROBEVAR}' 'B:$PROBEVAR' 'C:$$' 'D:${x%% *}' '$PROBEVAR'
    # prints: A:expanded B:$PROBEVAR C:$ D: expanded
    # plus the warning "Invalid environment variable name evaluates to
    # an empty string: x%% *"

So `${NAME}` anywhere in an argument and a whole-word `$NAME` are
replaced (an unknown or invalid name becomes the empty string), and
`$$` becomes `$`; `$NAME` inside a longer word stays literal. With
`--expand-environment=no` every argument arrives byte-identical. In
service mode (no `--scope`) the user manager has always done the same
expansion at spawn time, from the unit's environment; that is old
behaviour, not new in 262, but the same flag turns it off.

Any launcher that passes script text (`bash -c`, `sh -c`) or any
string it did not write itself through `systemd-run` can therefore
run a different command than it was given: loud failures, or false
passes (two trimmed values that both became "" compare equal). A
by-path exec of a file with plain arguments is unaffected.

Audit of every `systemd-run` call (both editions):

- Exposed: `cpuset-alloc run` (the user's command after `--`, scope);
  `gate.py` CPU containment (the gate command after `--`, and the
  `GATE_PREFLIGHT_COMMAND` shell text, scope); `dagrun run`'s re-exec
  into its outer scope (its own argv, for example an `--args` value);
  the `wrkslots run` fallback used when the D-Bus scope placement
  fails (the boxed command, scope); `herdr-run` with
  `broker: systemd-run` (service mode; `pane run` carries the command
  text).
- Not exposed: the scope probes, the Herdr server launch, the
  agentctl restore wake timer, the wrkslots image host handoff and
  FUSE mount units (fixed argv and paths), and `agentctl services
  refresh`, which replays a transient unit's own ExecStart in service
  mode, so the replay expands exactly as the original did.

# Design

- Each exposed call adds `--expand-environment=no` before its `--`,
  only when the installed `systemd-run` accepts it. Support is probed
  once per process and cached: `systemd-run --expand-environment=no
  --version` (exit 0 = supported; it starts no unit and needs no
  bus). Older systemd (before 254) rejects unknown options; there the
  call keeps its old argv and the tool logs one note saying the flag
  is unsupported (a scope there never expanded; service mode there
  did, and still does).
- One small helper per package (packages stay standalone), same probe
  and same note wording in the Rust and Python editions.
- `agentctl services refresh` is left unchanged on purpose: adding the
  flag would change what a replayed unit runs.
- The wrkslotsd scope-fallback plan mirror takes the same flag, so the
  editions keep one argv.

# Acceptance Criteria

- Per fixed call site and edition: a literal-echo probe test, where an
  argument holding `${PROBE}`, `$$`, `${x%% *}` and a whole-word
  `$PROBE` reaches the command byte-identical under a real
  `systemd-run` (skipped where none is usable), plus an argv test
  that the flag is present when the probe says supported and absent
  (with the note) when it does not.
- Existing argv-pinning tests updated, not weakened.
- Focused suites, `cargo fmt --check` and strict mypy green; one gate
  per fix commit.

# Notes

Exposure since the 262 upgrade on the reporting host: real gate rows
are unaffected (validate.py's dagrun argv and the gate command carry
paths, numbers and step names only); restore wake timers and services
refresh checks are unaffected; herdr-run there runs with the direct
broker. Remaining candidates are `cpuset-alloc run` and `wrkslots run`
fallback uses whose command text held `${...}` or `$$`.
