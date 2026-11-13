---
title: 'agentctl-console-effort-model: console /effort and /model with turn-boundary relaunch'
status: closed
priority: 1
issue_type: feature
assignee: mu-consolecmd
created_at: 2026-11-12T20:06:00.000000000+00:00
updated_at: 2026-11-12T20:06:00.000000000+00:00
---

# Description

Owner request (Slack 019, 021, 022): the headless console refuses every
`/command` except `/help`, `/interrupt` and `/quit`, so changing effort or
model means leaving the console for `set-effort` plus a manual restart, or a
profile change plus `restart --refresh-profile`. Add console `/effort` and
`/model` (direct and as a numbered local menu), a per-agent model override
with CLI `agentctl set-model`, and a prompt turn-boundary relaunch that
resumes the same session. The agent record is the single source of truth:
restore, revive, restart, turnover and status all read it, and no profile
refresh reverts an override.

# Acceptance Criteria

- Console `/effort LEVEL` does what `set-effort` does (harness-validated),
  then relaunches the headless wrapper at the next turn boundary with the
  session resumed (same session, token, queue and slot); a running turn is
  never cut (wait for the boundary or `/interrupt` first); declared and
  effective effort are echoed locally.
- A per-agent model override lives in the agent record (wins over profile;
  status shows declared vs effective). `agentctl set-model NAME MODEL` is
  validated against the harness adapter with no hard-coded model allowlist;
  console `/model NAME` uses the same path.
- Bare `/effort` or `/model` prints a numbered local menu (current declared
  and effective setting and its source, harness-valid options, current one
  marked), reads `1..N`, cancels on Enter or `q`, re-prompts on invalid
  input, and never queues anything; the agent inbox keeps delivering while
  the menu waits.
- `/help` lists `/effort` and `/model`; `agentctl status` shows both
  overrides. Plain `set-effort` keeps its apply-at-next-launch behaviour.
- After `/effort X` and a simulated restore/revive in a scratch registry the
  agent comes back at X with declared = effective = X.
- Python edition parity where the CLI or record shape is shared; the record
  stays additive and backward compatible; the Rust-only `set-model` is a
  named edition difference in the cross differentials.

# Design

Rust first. `reasoning_effort` stays the declared effort; a new additive
`model_override` field (also a nested-row common key both editions keep)
holds the model override, and `declared_model()` resolves override over
launch model. `refreshed_from_profile` keeps recorded overrides and builds
its launch spec from the declared values; revive replays apply them over
the captured spec, and headless launches apply them over the recorded
arguments, so every relaunch reads the record. `validate_model` asks the
harness adapter (structured launch) and checks only the name's shape.

The wrapper gains a `relaunch` control op carrying the declared arguments:
busy wrappers hold it until the turn completes, idle wrappers restart the
harness at once on the same session (no restart-limit count, no cut-short
notice). The console is a thin wrapper over `set_effort`/`set_model` plus
that op, with no console-only state; its menu is local input state only.

The Python edition gains the shared record field and status view and
refuses `set-model` by name as a Rust-only command; the differential pins
that as the `model-override` group.

# Notes

Owner replies 019, 021 and 022, 2026-11-12, via the coordinator. Related:
#1216 agentctl-effort-option (launch effort, `set-effort`, declared effort
on every relaunch) and #1023 (profile refresh on revive and restart).

## Resolution

Delivered: `model_override` record field with declared/effective status,
`agentctl set-model`, console `/effort` and `/model` (direct and numbered
menu) over the shared set paths, wrapper turn-boundary `relaunch`, and
profile-refresh/revive preservation so the record wins on every relaunch.
Python keeps the shared field and refuses `set-model` as the named
`model-override` differential difference. Gated with solo passes for the
load-flaked timing and differential checks.
