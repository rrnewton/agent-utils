# pidfd-process-ownership: generation-safe headless cleanup (design, 2026-10-19)

Status: design for #30 pidfd-process-ownership, implemented in the same branch.
Scope is the headless turn-runner runtime (`py/agentctl/foreign/`), which only
the Python edition drives, and the Rust edition's read-only deadness proof
for the same runtime (`rs/agentctl/src/subagents/turn_runner.rs`). The Rust
port of the runtime itself (M3a in
`2026-10-11-agentctl-rust-parity/SPEC.md`) inherits this model.

## 0. What was wrong

- `terminate_active_harness` read `active-harness.json`, compared the leader's
  start ticks, checked `getpgid(pid) == pid`, then called `killpg(pid)`. The
  number can be reused between the check and the call, and a group whose
  leader had already been reaped was not reachable at all.
- `terminate_runner_identity` did the same with `kill(pid)`, and a runner row
  without a start time was signalled with no generation check at all.
- In the runner, the watchdog thread called `killpg(proc.pid)` while the main
  thread could reap the leader in `proc.wait()`. Once the leader is reaped and
  its group is empty, the number is free.
- No record carried a boot identity. Start ticks are relative to boot, so a
  record that survives a reboot can match an unrelated process.
- Migration rollback called `terminate_runner_identity`,
  `clear_migration_markers` and `runner_identity_alive` unguarded inside the
  `except` block. Any of them raising replaced the original failure and skipped
  the rest of the rollback (the registry could keep naming the staged runner).
- `gc()` deleted rows and archived state inside one `registry_lock()`. An
  exception from harness cleanup skipped the registry write after state had
  already been moved.

## 1. Persisted same-boot identity

Every process identity the runtime records is the triple
`(boot_id, pid, start ticks)`, where `boot_id` is the canonical UUID from
`/proc/sys/kernel/random/boot_id` read in the same observation.

- **Runner.** Registry rows keep their 17 keys (older writers rewrite rows from
  their own schema and drop unknown keys). The boot identity lives in the
  sidecar `state/NAME/runner-identity.json`
  (`schema: agentctl-headless-runner/v1`, `name`, `created_at`, `runner_pid`,
  `runner_started_at`, `runner_boot_id`). It is written before the registry in
  the same locked save, like `presentation.json` and `permissions.json`, and
  applies only while all four keys match the row. A row an older writer
  rewrote with a new runner simply has no matching sidecar.
- **Staged replacement runner.** `MIGRATION_STAGED_<token>.json` gains
  `boot_id`. The migration commit copies it into the sidecar with the row.
- **Harness.** `active-harness.json` (`schema: agentctl-headless-harness/v1`)
  keeps the old keys (`runner_pid`, `runner_started_at`, `pid`, `started_at`)
  and adds `runner_boot_id`, `boot_id`, `process_group`, `generation` (128
  random bits), `containment` (`process` or `cgroup`) and, for cgroup
  containment, `cgroup` (the absolute path of the per-turn directory).

A recorded boot that differs from the current one (read twice, stable) is a
positive death certificate. Unreadable or malformed boot or procfs evidence is
never treated as death; it refuses with `runner_identity_unknown`, as before.

## 2. Signalling only through a pidfd or an owned cgroup

No code in the runtime calls `kill`, `killpg` or `os.kill` with a number any
more (the test `test_runtime_never_signals_by_number` greps the package).

- **Pinning.** `pidfd_open(pid)`, then read `/proc/pid/stat` and require the
  recorded start ticks and boot, then poll the pidfd and require "not exited".
  A process that is alive after the read was alive during it, so the read
  described the pidfd's process. Every later signal is `pidfd_send_signal`,
  which cannot reach a different process.
- **Runner stop** (`terminate_runner_identity`): pin, `SIGTERM`, wait on the
  pidfd for the grace period, `SIGKILL`, wait a bounded time.
- **Harness spawn.** The runner starts the harness behind the existing launch
  gate (`_launch_gate.py`): a new session, `PR_SET_PDEATHSIG`, and no exec until
  the runner has opened a pidfd on the gate (its own unreaped child, so the
  pidfd is exact), moved it into the per-turn cgroup if one is used, and
  written `active-harness.json` durably. No harness process can exist that is
  not covered by a durable record, and a runner that dies before activation
  takes the inert gate with it.
- **Per-turn cgroup** (`AGENTCTL_HEADLESS_CGROUP=auto|require`, default `off`):
  a leaf `agentctl-harness-<generation>` under the runner's own cgroup v2
  directory. Cleanup writes `1` to `cgroup.kill` (atomic, and the kernel kills
  children forked during the kill), waits for `populated 0` in
  `cgroup.events`, and removes the directory. `auto` falls back to process
  containment when the directory or the move is refused (a non-delegated
  scope); `require` fails the turn instead. Default off follows the parity
  spec: writing into a unit that systemd did not delegate is the operator's
  choice.
- **Process containment** (default). Members of the harness tree are found by
  three predicates, each with its own proof, and each candidate is pinned and
  re-checked after the pidfd is open:
  1. its initial environment carries `AGENTCTL_HARNESS_GENERATION=<generation>`
     (only this turn's tree can carry those 128 bits; checked for same-uid
     processes younger than the leader);
  2. its session is the leader's session, *and* the pinned leader is still
     present after the candidate's re-check (a session id cannot be recreated
     while its leader holds the number);
  3. its parent is the runner, *and* the pinned runner is still alive after the
     re-check. The runner is a child subreaper (`PR_SET_CHILD_SUBREAPER`), so
     orphans of the harness tree, including double-forked `setsid` escapees,
     become its children.
- **Unsupported pidfds** (no `os.pidfd_open`, or `ENOSYS`). Inside the runner
  the leader is its own unreaped child: it is signalled by number only under
  the reap lock and only while `waitid(WNOWAIT)` shows it unreaped, so the
  number cannot have been recycled. Other members are reached by repeating
  that on orphans the subreaper adopts. Outside the runner nothing is
  signalled: `stop` writes `STOP`, waits, and refuses with
  `pidfd_unsupported` instead of guessing.

## 3. Leader-dead, child-live groups

Process groups are no longer the unit of signalling. When the leader is gone,
predicate 2 is unavailable (its number may have been recycled) and is not
used; predicates 1 and 3 still prove membership. A same-session process
without the generation and without the runner as parent is reported as an
unproven possible member and is not signalled. Inside the runner the leader's
exit is observed with `waitid(P_PID, WEXITED | WNOWAIT)` and the leftovers
are cleaned *before* the leader is reaped, so predicate 2 stays valid for the
whole turn-end sweep: the zombie leader pins its number. `OwnedHarnessProcess`
(a `Popen` subclass) makes `wait()` and `poll()` do exactly that, so
`communicate()` inherits it.

Leftovers are cleaned at the end of every turn, not only on stop or timeout:
the ownership record covers one turn, and a record removed while members live
would leave them untracked. The transcript notes how many leftovers were
killed.

## 4. Fork during census

Cgroup containment: `cgroup.kill` is atomic with respect to fork.

Process containment: stop the world, then kill. Each round pins and `SIGSTOP`s
every member found (the leader first, so predicate 2 stays valid); stopped
processes cannot fork. Rounds repeat until one finds no member that is not
already stopped. Then every pinned member gets `SIGKILL`, the leader last.
A process forked between a census and its `SIGSTOP` inherits the generation and
the session, so the next round finds it. A tree that forks faster than the
rounds converge hits the deadline; everything found is still killed and the
report says the census did not converge.

## 5. D-state and timeouts

Every wait is bounded (`SUBAGENTS_HARNESS_KILL_WAIT`, default 10 s, for the
tree; the caller's grace plus the same bound for a runner). A process in
uninterruptible sleep keeps its pidfd unreadable, so it is reported, by pid and
procfs state, as remaining. Incomplete cleanup:

- keeps `active-harness.json` (and the cgroup), so a later attempt can finish;
- raises `harness_cleanup_incomplete` (or `runner_stop_incomplete`) to
  `stop`, `down`, migration and `gc` callers, which do not archive;
- inside the runner, is noted in the transcript, and the next spawn retries the
  previous record before starting a new harness; it refuses the turn if the
  previous tree still has members.

## 6. Migration rollback paths

Each rollback step runs on its own and its failure is collected, never raised
over the original error:

| Path | Steps, in order |
|---|---|
| `migrate_agent` | stop the staged runner (pinned), close the destination presentation, restore the registry row and sidecar, clear markers, re-check the source |
| `_migrate_legacy_tmux_source` | the same, then restart the stopped legacy source |
| `_convert_headless_to_tui` | close the TUI destination, restore a retired source, resume source intake |
| `_convert_legacy_tmux_headless_to_tui` | close the TUI destination, restore the source, resume intake |

The raised error always keeps the original failure in its message and as
`__cause__`; its code names the rollback outcome (`*_rolled_back`,
`*_rollback_failed`). When the registry cannot be restored, the remaining
steps still run and are listed. The registry restore puts back the source's
boot identity with its pid and start time.

## 7. Legacy state policy

- A runner identity without a matching sidecar (written before this change, or
  rewritten by an older writer) is *legacy*. It is still used for liveness as
  before. It is signalled only when, under a pidfd, its start ticks match and
  `/proc/pid/cmdline` proves it is this worker's runner (`agent_runner.py`
  followed by the worker name). A legacy identity with no start time is never
  signalled (previously it was signalled by number).
- A legacy `active-harness.json` (no `boot_id` or `generation`) is never
  signalled directly. If its leader is provably gone the stale record is
  removed. If its recorded runner is alive and proven, that runner is stopped
  and its own watchdog cleans its group. Otherwise cleanup refuses with
  `harness_identity_legacy`, naming the pid, without archiving.
- The Rust deadness proof applies the same rules: a recorded boot that differs
  proves death; no boot falls back to start ticks, as before.

## 8. Both editions

The Python edition owns every signal. The Rust edition never signals; its
`stop --expected-token` deadness proof now reads the boot identities, the
runner sidecar, the per-turn cgroup and the generation census, so a worker
whose harness leader died while children live is not retired. Both editions'
unit tests load the same corpus, `rs/agentctl/testdata/headless-ownership/`,
which pins the verdict (dead, alive, inconclusive) for each combination of
records and procfs state.

## 9. Tests

Real-process tests start only short-lived children of the test and signal
nothing else: PID reuse is simulated by recording a stale start tick or boot
for a live child and asserting it survives; leader-dead/child-live, `setsid`
escapee, double fork, a forking tree during cleanup, crash recovery from
another process, and both containment modes run against real processes.
Fault injection covers unsupported pidfds, D-state (an exit that never comes),
and a failure at every migration rollback step. Negative controls: each new
test fails on the parent commit.
