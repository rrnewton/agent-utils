---
title: 'wrkslots-live-seal-reported-as-interrupted: a live validation-batch seal is refused as interrupted and pointed at recover'
status: open
priority: 1
issue_type: bug
labels:
- wrkslots
created_at: 2026-11-16T15:45:41-05:00
updated_at: 2026-11-16T15:45:41-05:00
---

# Description

[watermelon-full-preview] Consumer report (2026-11-16): `wrkslots adopt` refuses with "interrupted validation-batch seal ... run 'wrkslots recover' first" while the `remove-validate-batch` that owns the seal is still alive (its recorded pid live, nine minutes into the batch). The wording calls a running operation interrupted and invites a destructive `recover` against a live batch.

The seal gate (`_refuse_partial_state` in `py/wrkslots/cli.py`, and the matching Rust checks) refuses on any seal journal without asking whether its recorded actor is alive. The liveness proof already exists in the same file: `_validate_batch_seal_orphaned` judges a seal orphaned only when its actor ran on this machine identity and boot and its pid is absent or names another process generation (`_process_state`), and classification already reports that distinction in its structured output. The refusal text just never uses it.

# Design

- Every command that meets a validation-batch seal checks its owner with the same liveness proof the rest of wrkslots uses (`_process_state` on the seal journal's recorded actor, same machine and boot), in both editions.
- Owner proven alive: the refusal says the seal is "in progress (pid N, since T)" naming the recorded pid and the journal's creation time, then waits under `--wait-lock` where the command takes one, or refuses without suggesting `recover`.
- Owner proven dead: only then does the refusal call the seal interrupted and suggest `wrkslots recover`.
- `recover` itself refuses a seal whose owner is proven alive, instead of retiring it.

# Acceptance Criteria

- A deterministic test: with a seal whose recorded actor is a live process, `adopt` (and each other seal-gated command) refuses naming "in progress" with the pid, and never suggests `recover`; the seal file is unchanged.
- A deterministic test: with a seal whose recorded actor is proven dead, the refusal still calls it interrupted and suggests `recover`.
- A deterministic test: `recover` refuses a live-owned seal and retires a dead-owned one.
- Python and Rust editions agree (paired or differential tests). Live and dead actors are real processes (a sleeper and an exited child), never wall-clock budgets.

# Notes

Related: the seal-ownership tests in `py/wrkslots/tests/test_validate_batch_seal_ownership.py` already distinguish live and dead actors for classification and recovery; the refusal wording does not. Land separately from #1735 wrkslots-remove-holds-state-lock-through-salvage when ready.
