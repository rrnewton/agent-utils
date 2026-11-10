# #238/#545 herdr-subagents-port-map — Track B step B3 (scoping + first slice)

Base: slot `rsctl` HEAD `fa13a988` · repo `agent-utils` · Date 2026-11-07
Python total: **8,265 LOC** (`py/agentctl/foreign/`).

## 1. Python inventory (wc -l)

| Module | LOC | Role |
|---|---|---|
| `lib.py` | 4,049 | Registry, backend/mode selection, up/send/read/status/down/reset/migrate core, inbox dirs, durable JSON, active-harness read/write |
| `ownership.py` | 1,510 | `(boot_id,pid,starttime)` identity, pidfd pin, HarnessRecord parse/validate, cgroup v2 per-turn containment, `kill_harness_tree` |
| `agent_runner.py` | 969 | Runner daemon: dispatch turns to codex/agy/muse, spawn owned harness, session capture |
| `mcp/server.py` | 474 | MCP server |
| `agent_turn.py` | 252 | `turn` command |
| `agent_keeper.py` | 182 | Keeper / supervisor |
| `mcp/adapter.py` | 160 | MCP adapter |
| `agent_up.py` | 92 | `up` |
| `agent_down.py` | 78 | `down` |
| `agent_status.py` | 77 | `status` |
| `agent_send.py` | 71 | `send` |
| `agent_inbox.py` | 58 | `inbox` |
| `agent_read.py` | 58 | `read` |
| `policy.py` | 57 | Launch quota policy |
| `agent_migrate.py` | 50 | `migrate` |
| `cli.py` | 48 | Dispatcher (`up/send/read/status/down/inbox/reset/migrate/backend/turn/keeper/mcp`) |
| `agent_backend.py` | 38 | `backend` |
| `agent_reset.py` | 35 | `reset` |
| `__main__.py` | 5 | Entry |
| `__init__.py`, `mcp/__init__.py` | 2 | Packages |
| `tests/` | — | `test_ownership.py`, `test_agent_runner.py`, `test_backends.py`, `test_mcp_server.py` |

## 2. Harnesses (adapters)

`SUPPORTED_HARNESSES = ("codex", "agy", "muse")` in `lib.py:65`. `SUPPORTED_BACKENDS = ("tmux", "herdr")`. `SUPPORTED_MODES`: headless default; `--mode tui` codex-only (`lib.py:2556-2559`).

| Harness | Runner turn fn | Invocation | Session-id ownership |
|---|---|---|---|
| `codex` | `_run_codex_turn` (`agent_runner.py:489`) | First: `codex exec ... -` (captures new id); later: `codex exec resume <id> ... -`; `_build_codex_argv:284` | Harness-assigned, captured from `--json` events |
| `agy` (Antigravity) | `_run_agy_turn:580` | `_build_agy_argv:312` + turn log `_agy_turn_log_path:308` | Captured from log (`_capture_agy_session_id:400`); outcome classified `_classify_agy_outcome:446`; model note `_agy_model_note:407` |
| `muse` | `_run_muse_turn:650` | `_build_muse_argv:332` around caller-owned session id; pre-`exec` TUI-opt ordering guard | Caller-owned UUID minted at start (`lib.py:2593`, `3006`); strict JSONL validation (limits `_MUSE_MAX_*`, monotonic seq, terminal-outcome match, workspace epilogue `_is_muse_workspace_epilogue:354`) |
| `other` / generic | `_run_turn:818` dispatches on `rec.harness` | — | Unknown harness rejected by `require_supported_harness` (`lib.py:171-178`) |

TUI seam: `harness_tui("codex")` headless→TUI (`lib.py:2798`); `_run_turn:818` is the dispatch point.

## 3. Commands (`py/agentctl/foreign/cli.py:17`)

`COMMANDS = ("up", "send", "read", "status", "down", "inbox", "reset", "migrate", "backend", "turn", "keeper", "mcp")`.

Dispatcher: `cli.py:main` special-cases `--version` / `--userguide`, parses first arg as command, dynamically imports `agentctl.foreign.agent_<cmd>` (or `mcp.server` for `mcp`), rewrites `sys.argv[0]` to `herdr-subagents <cmd>`, calls `module.main()`.

Thin CLI wrappers (`agent_up.py` 92, `agent_send.py` 71, `agent_read.py` 58, `agent_status.py` 77, `agent_down.py` 78, `agent_inbox.py` 58, `agent_reset.py` 35, `agent_migrate.py` 50, `agent_backend.py` 38) parse args and call `lib.py` functions. `agent_turn.py` (252) and `agent_keeper.py` (182) carry more logic.

## 4. Shared core

| Core | File | Contents |
|---|---|---|
| Registry/state | `lib.py` | `AgentRecord`, `AgentStatus`, `UpResult`, `SendResult`, `ReadResult`, `DownResult`, `MigrationResult`, …; state under `HERDR_SUBAGENTS_HOME` (`BASE`), `backend.json`, `project_defaults.json`; `registry_lock`/`read_registry`/`agent_dir`/`inbox_dir`/`processed_dir`/`inflight_dir`/`failed_dir`; `_write_durable_json`; `record_active_harness`/`read_active_harness` bridging to `ownership.parse_harness_record` (`lib.py:960-997`) |
| Ownership | `ownership.py` | `ProcessIdentity` (`pid`, `started_at`, `boot_id`), `HarnessRecord` (`runner`, `leader`, `process_group`, `generation`, `containment`, `cgroup`), `parse_harness_record`, `pin` (`ALIVE`/`GONE`/`UNPROVEN`), `PinnedProcess`/`ChildHandle`, `stop_process`, `kill_harness_tree`, `CleanupReport`, `current_boot_id`, `valid_boot_id`, cgroup helpers, `set_child_subreaper` |
| Runner | `agent_runner.py` | `main:901`; `_spawn_harness:187` → `ownership.spawn_owned_harness`; `_owned_harness:101`, `_finish_ownership:150`, `_clear_previous_harness:167`, `ownership.set_child_subreaper` |
| Policy | `policy.py` (57) | Launch quota checked via `require_harness_quota` (`lib.py:181`) |

Key invariant (from `ownership.py` docstring): a PID alone never identifies a process; identity is the triple `(boot_id, pid, start_ticks)`. Signals go only through pidfd handles or the per-turn cgroup (`cgroup.kill`); the lone numeric-signal path is the runner signalling its own unreaped children under its reap lock. Harness-tree membership is *proved* (generation env, leader session, or runner-parent), never assumed.

`HarnessRecord` / `active-harness.json` schema `agentctl-headless-harness/v1` (`HARNESS_SCHEMA`); `RUNNER_SCHEMA = agentctl-headless-runner/v1`; legacy records (no `schema` key) parse as `containment=legacy`, `generation=None`, `process_group=pid`. cgroup dir name is `agentctl-harness-<generation>` where generation is 32 lowercase hex chars.

## 5. `sessions.py` → foreign routing (`worker_rpc`)

`py/agentctl/sessions.py` (467 LOC) — `Sessions(ManagedAgents)` is the single lifecycle authority. For headless turn-runner records (`record.runtime_home` set), `_worker` (`sessions.py:34-55`) spawns `[sys.executable, worker_rpc.py]` with `HERDR_SUBAGENTS_HOME=runtime_home` and one JSON request on stdin; 90 s timeout (900 s for `migrate`). The adapter is *not* retried after a committed request — uncertainty is reported, not retried.

`worker_rpc.py:43-85` dispatches:

| RPC action | `lib.py` call |
|---|---|
| `start` | `bring_up_agent(name, cwd, harness, model, brief, backend, mode="headless", harness_args)` |
| `status` | `status_snapshot(name, run_gc=False)` |
| `send` | `send_message_to_agent(name, text, model)` |
| `read` | `read_agent_output(name, mode, since_turn, tail)` |
| `stop` | `bring_down_agent(name, archive=True)` |
| `reset` | `reset_agent_context(name)` |
| `migrate` | `migrate_agent(name, to_backend, to_mode)` (`interactive`→`tui`) |
| `repair` | `recreate_window(name)` |
| `pause`/`resume` | `automation-paused` marker file |

Response: `{"result": ..., "record": ...}` (record re-read after the op). Errors → `{"error", "code"}` with exit 1; `AgentOperationError.code` preserved.

Headless gate in `start_session` (`sessions.py:115-116`): `harness in ("codex","agy","muse")`, `backend in ("herdr","tmux")`, `mode == "headless"`; `resume`/`workspace_id`/`environment` rejected for headless. Capabilities (`sessions.py:57-80`): headless adds `final-answer`, `reset`, `migrate`, `repair`; foreign adapters (`herdr-foreign` etc.) add `drain`, `goal`, `bind-session`, `anchor`, `rename`.

## 6. Slice order (Rust port)

Each slice independently testable; no CLI wiring until the last slice.

| Slice | Scope | Size est. | Depends on |
|---|---|---|---|
| **B3-1 (this slice)** | `rs/agentctl/src/foreign/ownership.rs`: `ProcessIdentity`, `HarnessRecord`, `Containment`, `parse_harness_record`, `HarnessRecord::document`, `valid_boot_id`, `valid_generation`, `valid_ticks`. Read/validate parity with `ownership.py:499-555` and `HarnessRecord.document:473-490`. Unit tests only; no CLI wiring, no live workers. | ~330 LOC Rust + ~200 LOC tests | — |
| B3-2 | `current_boot_id` (read `/proc/sys/kernel/random/boot_id`), `read_stat`/`ProcessStat` parse (`py/agentctl/procstat.py` parity), procfs scan helpers | ~250 LOC | B3-1 |
| B3-3 | pidfd pin (`pin` → `Alive`/`Gone`/`Unproven`), `PinnedProcess` (signal/wait/close), `stop_process`. Linux-only (`pidfd_open` via `libc`). | ~400 LOC | B3-2 |
| B3-4 | `lib.py` registry subset: `AgentRecord` serde, registry lock/read, `active_harness_path`/`read_active_harness`/`record_active_harness` bridging to B3-1, durable JSON write (tmp+rename+fsync) | ~600 LOC | B3-1 |
| B3-5 | cgroup v2 helpers (`cgroup_mode`, mount/cgroup discovery, `_kill_cgroup`) + `kill_harness_tree` census. Requires B3-3. Highest-risk slice; gate on parity tests vs `test_ownership.py`. | ~700 LOC | B3-3, B3-4 |
| B3-6 | `agent_runner.py` turn dispatch: argv builders for codex/agy/muse (runner already owns most harness-specific parsing; Muse JSONL validator is the bulk). | ~900 LOC | B3-4, B3-5 |
| B3-7 | CLI wrappers `agent_up/send/read/status/down/inbox/reset/migrate/backend/turn/keeper` + `cli.py` dispatcher as `rs/bin/herdr-subagents` (explicitly *not* in B3-1). | ~500 LOC | B3-4..B3-6 |
| B3-8 | `mcp/server.py` + `mcp/adapter.py` (634 LOC py). Defer until B3-7 lands. | ~600 LOC | B3-7 |
| B3-9 | `sessions.py` seam: Rust `agentctl` headless path calls the Rust worker instead of `worker_rpc.py`; Python `worker_rpc` kept as fallback until parity proven. | — | B3-7 |

Total Rust estimate: ~4,300 LOC for lib+ownership+runner (Python 6,528) plus ~1,100 for CLI+MCP. Harnesses port in runner order: **codex first** (simplest argv, harness-assigned session), **agy second** (log-scraped session/outcome), **muse last** (caller-owned session + strict JSONL protocol validation).

## 7. B3-1 scope (this turn)

New module `rs/agentctl/src/foreign/` (`mod.rs` + `ownership.rs`), registered as `pub mod foreign;` in `rs/agentctl/src/lib.rs`. No CLI wiring, no `rs/bin/herdr-subagents`, no live-worker contact.

Covered behaviour (parity with `ownership.py`):

- `ProcessIdentity { pid, started_at, boot_id }` (frozen-dataclass equivalent: immutable fields, `Clone+PartialEq`).
- `HarnessRecord { runner, leader, process_group, generation, containment, cgroup }` + `legacy()` + `document(recorded_at)` emitting exactly the keys of `ownership.HarnessRecord.document` (schema, runner_pid, runner_started_at, runner_boot_id, pid, started_at, boot_id, process_group, generation, containment, recorded_at, cgroup only when `Some`).
- `parse_harness_record(&serde_json::Value) -> Result<HarnessRecord, ForeignOwnershipError>` with error code `harness_identity_unknown` and the same acceptance/rejection rules: non-object rejection; `pid`/`runner_pid` process-number validation (`1..=2^31-1`, `pid > 1`); `started_at` string requirement; legacy (no-`schema`) fallback; `HARNESS_SCHEMA` check; boot-UUID validation; 32-hex generation; process-group + containment validation; cgroup path rules (absolute, no `..`, basename `agentctl-harness-<generation>`); process-containment-with-cgroup rejection.
- `valid_boot_id(&str) -> bool` (canonical lowercase UUID), `valid_generation` (32 lower-hex), `valid_ticks` (canonical non-negative integer, no leading zeros except `0` itself, ≤20 digits).

Tests: `#[cfg(test)]` unit tests inside `ownership.rs` (legacy parse, v1 process/cgroup parse, document round-trip, and each rejection class). Run: `cargo test -p agentctl foreign::ownership`, `cargo clippy -p agentctl --all-targets`, `cargo fmt --check`.

## 8. B3-2 scope (kernel evidence)

New module `rs/agentctl/src/foreign/procstat.rs` (`ProcessStat`, `parse_process_stat`, byte-parity with `py/agentctl/procstat.py`: opaque `comm` up to the last `)`, state allowlist, field-22 starttime, pid/ppid/pgrp/session ranges) plus procfs readers in `ownership.rs` at parity with `ownership.py:120-178`: `current_boot_id`/`current_boot_id_at` (128-byte cap, canonical-UUID validation, `runner_identity_unknown` on unreadable or malformed evidence), `read_stat`/`read_stat_at` (`None` only for positive disappearance — ENOENT/ESRCH; malformed or pid-mismatched records error), and `scan_processes`/`scan_processes_at` (numeric entries only, vanished/unreadable skipped, unlistable root errors). The `_at` forms take an explicit procfs root so tests run against a fake tree; the plain forms read `/proc`. `carries_generation` and `_same_user` stay with the B3-5 census that uses them. Still no CLI wiring, no pidfd pinning (B3-3), no live workers.

Tests: `#[cfg(test)]` unit tests in both files (kernel-record shapes including `comm` with embedded `)`/spaces/non-UTF-8, every state byte, range rejections; fake-procfs boot/stat/scan cases; one Linux live-procfs self-check). Run: `cargo test -p agentctl foreign`, `cargo clippy -p agentctl --all-targets`, `cargo fmt --check`.

## 9. B3-3 scope (pidfd pinning)

In `ownership.rs`, at parity with `ownership.py:199-449`: `pidfds_supported`, `pin`/`pin_at` returning `Pin { verdict: Alive|Gone|Unproven, reason, process }` with the Python decision order (pid range, boot valid/same with change re-check, ticks shape, no-pidfd fallback verdicts, pidfd open, stat re-read, start-tick equality, dead states Z/X/x, legacy proof, exit re-poll), `PinnedProcess` (`pid`/`starttime`/`descriptor`, `exited`, `wait`, `send` via `SYS_pidfd_send_signal`, consuming `close`; the fd is an `OwnedFd`, so non-alive pins release on drop), and `stop_process`/`stop_process_at` (SIGTERM, grace, SIGKILL, `runner_stop_incomplete`; unproven refuses with `runner_identity_unproven`/`pidfd_unsupported` and no signal; `$SUBAGENTS_HARNESS_KILL_WAIT` default read per call). pidfd syscalls are Linux-only; other targets report pidfds unsupported and take the same refusal path Python takes without them. `ChildHandle` (the parent-pinned fallback) stays with `spawn_owned_harness` in the runner slices. No CLI wiring, no tree cleanup (B3-5).

Tests: Linux `#[cfg(test)]` integration tests inside `ownership.rs` against real spawned children only (`sleep`, a TERM-ignoring shell), mirroring `test_ownership.py` pin/stop cases including the adversarial reused-pid/earlier-boot no-signal proofs and exit-status signal assertions. Run: `cargo test -p agentctl foreign`, `cargo clippy -p agentctl --all-targets`, `cargo fmt --check`.

## 10. B3-4a scope (layout + active-harness bridge)

New module `rs/agentctl/src/foreign/registry.rs`, the first half of B3-4: `foreign_base` (`$HERDR_SUBAGENTS_HOME` → `$XDG_STATE_HOME/herdr-agent/foreign` → `~/.local/state/herdr-agent/foreign`, with `~` expansion; no symlink resolution, unlike Python's `.resolve()`), `require_valid_name` (`invalid_name`), layout paths (`state_dir`, `agent_dir_at`, `inbox/processed/inflight/failed_dir_at` — the Rust forms validate the name where Python's `agent_dir` trusts its caller), `write_durable_json` (exclusive `.{name}.{pid}.{hex6}.tmp`, fsync, rename, directory fsync; compact `serde_json` bytes + newline rather than Python's spaced separators — semantic parity), and the active-harness bridge wiring B3-1 to disk: `active_harness_path_at`, `record_active_harness_at` (incomplete identity refuses with the exact Python message; `recorded_at` is caller-supplied until the runner slice owns `now_iso` parity), `read_active_harness_at` (`None` only for a missing file; unreadable/malformed is `harness_identity_unknown`), `remove_active_harness_at` (generation-exact, unreadable kept). Deferred within B3-4: `AgentRecord` serde + registry lock/read (`registry.json`), `_owned_by`/`terminate_active_harness` (needs B3-5), `_harness_lock`.

Tests: `#[cfg(test)]` inside `registry.rs` against a fake base tree (name shapes incl. traversal attempts, layout, base resolution, record→read→remove round-trip with no temp leftovers, refusal codes/messages, durable overwrite). Run: `cargo test -p agentctl foreign`, `cargo clippy -p agentctl --all-targets`, `cargo fmt --check`.

## 11. B3-4b scope (AgentRecord + locked registry)

Completes B3-4 in `registry.rs`: `AgentRecord` with Python `from_dict`/`to_dict` semantics (required-vs-omissible keys, backend inference from a Herdr tab id, supported-mode check, harness-args validation, string/integer scalar coercions, runner identity sidecar omitted from the row), sidecar load/save for presentation identity, Codex permission policy, and runner boot identity (matching name/created/target or runner fields exactly; corrupt runner sidecars stay legacy, as in Python), plus `_load_unlocked` TUI-repair detection, `_save_unlocked` (sidecars first, sorted pretty JSON), `read_registry` (shared lock, exclusive only for repair), and `registry_lock`/`with_registry_lock_at` (exclusive `flock`; explicit save, drop-without-save discards like an exceptional Python context exit). Malformed registry/row/presentation data that Python exits on via `die()` returns structured `registry_invalid` errors with the Python messages; filesystem failures use `registry_io`; the Python structured codes (`invalid_permission_policy`, `unsupported_backend`, `unsupported_mode`) are preserved. Sidecar paths validate worker names where Python trusts them, so a corrupt name cannot traverse out of an agent directory. `terminate_active_harness` and `_resolve_legacy_harness` remain with B3-5; `_harness_lock` remains with the runner slices.

Tests: fake-base `#[cfg(test)]` in `registry.rs` for legacy defaults/backend inference, missing/invalid rows, permission sidecar generations, sorted round-trip with all sidecars, stripped-TUI repair, failed-lock discard, and corrupt registry shapes. Run: `cargo test -p agentctl foreign`, `cargo clippy -p agentctl --all-targets`, `cargo fmt --check`.

## 12. B3-5 scope (cgroup helpers + harness-tree cleanup)

In `ownership.rs`, at parity with `ownership.py:181-197,293-338,563-1214`: `ChildHandle` + `ProcessHandle` (the parent-pinned fallback deferred from B3-3), `cgroup_mode` (`CgroupMode`, `invalid_cgroup_mode`), cgroup v2 discovery (`own_cgroup_directory`, mount/proc-cgroup parsing) and `_kill_cgroup` (cgroup.kill, with the pre-5.14 per-member pin-and-re-prove fallback), `CleanupReport` (`complete`/`describe`), child-subreaper state (`set_child_subreaper`, `reap_adopted_children`), the stop-the-world census (`kill_harness_tree`: SIGSTOP the leader, pin+stop proven members by live-session/runner-or-member-parent/generation proofs with post-pin anchor re-checks, SIGKILL, wait, re-census; emergency SIGKILL of proven members on census error; no-pidfd runner peel of its own unreaped children; `pidfd_unsupported` only when candidates may live), and the read-only `recorded_process_alive`/`tree_alive`. `registry.rs` gains `terminate_active_harness` + the legacy resolve-never-signal policy (`harness_cleanup_incomplete` keeps the record). Every public entry has an `_at(proc_root)` form; signals still go only through pidfds, the per-turn cgroup, or the parent-pinned unreaped-child path. Registry cgroup I/O failures use `harness_cgroup_unavailable` where Python propagates raw `OSError`. Spawning (`spawn_owned_harness`, `_create_cgroup`, launch gate) remains B3-6.

Tests: Linux `#[cfg(test)]` against fake procfs/cgroup trees and real spawned children only (47 foreign tests total): ChildHandle WNOWAIT semantics, cgroup discovery/fake kill-file, legacy + earlier-boot refusals, the recycled-session adversarial case (unproven, untouched), an external cleanup reaching session + setsid-generation members of a real setsid tree, recorded/tree-alive exactness, and registry terminate (exact record killed+removed; legacy alive refused untouched, stale removed). Test isolation note: cleanup tests never record the shared test binary as the runner — child-of-the-runner would otherwise claim sibling tests' children. Run: `cargo test -p agentctl foreign`, `cargo clippy -p agentctl --all-targets`, `cargo fmt --check`. Not a substitute for the stack's full gate + Python `test_ownership.py` runner-fixture cases, which stay Python-side until B3-6 spawns in Rust.

## 13. B3-6 scope (owned spawn + runner dispatch core)

In `ownership.rs`: `OwnedHarnessProcess` (sweep-before-reap `wait`/`poll`, `kill_tree`, `sweep_after_exit`, verified-handle `send_signal`/`terminate`/`kill` with `harness_not_bound`, `finish`, `wait_with_output` with drainer threads, clonable leader handle for a watchdog) and `spawn_owned_harness`/`spawn_owned_harness_at` (resolve-then-gate: executable resolution with `ENOENT` as an I/O error, random 32-hex generation env, session-leader gate child behind this binary's existing `--chat-launch-gate` entry — configurable via `SpawnOwnedHarnessOptions.gate_prefix` — stable-identity checks, pidfd or parent-pinned child handle, readiness, per-turn cgroup create/enter with `require`/`auto` fallback semantics, durable `record` before the one-byte activation, and Python-shaped failure cleanup: close activation first, SIGKILL only through the exact handle, bounded reap, cgroup removal). Reap serialization is by `&mut self` ownership in Rust rather than Python's `RLock`; a watchdog signals through `leader_handle_try_clone`. In `registry.rs`: `Message::from_dict`/`from_path`. New `runner.rs`: env-resolved harness bins and turn timeout, Codex/Antigravity/Muse argv builders (Muse runner-owned-option refusals keep the Python `ValueError` texts as `String` errors), Codex pane-echo + session/error observation, Antigravity session/model/log parsing and quota/auth classification, and the bounded Muse JSONL validator (`MuseTurnValidator` with written/kill line outcomes, one strict workspace epilogue, finish classification). The runner daemon loop itself (inbox intake lock/consume/quarantine, transcript + event-log writing, status/session registry writes, watchdog thread, staged migration, `main`) remains for the CLI/session slices (B3-7/B3-9) together with the lib helpers it needs (`write_event`, `next_pending_message`, staged-runner and migration markers).

Also fixes a B3-3 `fd_readable` deadline truncation found by a timing flake (poll timeouts round up and loop to the deadline, as Python does). Dependency note: `regex` moves from dev-dependencies to dependencies of the `agentctl` crate (already in the workspace lock). Tests: 57 foreign tests total (three consecutive runs), including real gated spawns against a compiled Rust gate double (the production gate is the agentctl binary itself, which a test binary cannot be; an interpreter launcher that forks breaks the exact-parent proof — observed with this host's `python3` wrapper), record-failure never-runs, stdin echo, missing-executable `ENOENT`, argv exactness for all three harnesses, and Muse validator accept/reject streams. Signalling audit: every signal in tests targets a process the test itself spawned, by exact pidfd/child handle or `Child::kill` on its own `Child`; no pattern kills. Run: `cargo test -p agentctl foreign`, `cargo clippy -p agentctl --all-targets`, `cargo fmt --check`.

## 14. B3-7a scope (foreign lifecycle subset)

New module `rs/agentctl/src/foreign/lifecycle.rs`: per-worker paths (transcript, last answer, runner pid/stderr, STOP), `now_iso`, `ensure_agent_dirs`, inbox (`msg_filename`, `enqueue_message_at` with registry-locked sequence allocation and the pause-marker refusal before allocation burns a sequence, unknown-agent and `agent_migrating` codes, `next_pending_message`, `pending_count`), the flock-guarded central event log (`write_event_at`), the per-worker harness lock (0600 `O_NOFOLLOW` flock guard), migration pause/staged-runner markers (write/read/acknowledge/clear/wait, boot+tick re-validation on read), and runner liveness/termination (`pid_start_time`, `pid_alive`, `runner_identity_alive`, `terminate_runner_identity` with the cmdline legacy proof accepting `agent_runner.py` or `herdr-subagents`, `terminate_runner` with the legacy-defer-watchdog order). Adds `chrono` (already in the workspace lock) to the agentctl crate for local-time ISO stamps. The runner daemon itself is B3-7b; CLI wrappers + `rs/bin/herdr-subagents` are B3-7c.

Tests: 60 foreign tests total (3 new: enqueue ordering + pause refusal without sequence burn, event/marker round-trips, liveness + SIGTERM termination of an own spawned stand-in). Run: `cargo test -p agentctl foreign`, `cargo clippy -p agentctl --all-targets`, `cargo fmt --check`.

## 15. B3-7b scope (runner daemon)

New module `rs/agentctl/src/foreign/daemon.rs`: the per-agent turn-runner (`run_runner`/`run_runner_default`) — SIGTERM flag + SIGCHLD reset, child-subreaper adoption, existing-runner refusal, runner identity publication, staged-activation wait, RUNNER UP/DOWN sentinels, inflight recovery, and the poll loop (stop marker, migration pause acknowledgement, automation pause, idle adopted-child reaping). Intake claims under `.headless-turn.lock` with fsync + move to processed, quarantining invalid or duplicate requests with durable sidecars and a `turn_quarantined` event. Turns execute on the B3-6 core: Codex streaming with session capture and answer-file publishing, Antigravity with session capture/model note/quota-auth classification, Muse with the bounded validator fed raw lines (transcript write semantics preserved), each inside the watchdog (stop, turn timeout, 1s leftover sweep through a cloned exact leader handle; reap serialization by `&mut` ownership rather than a reap lock). B3-7c remains: CLI wrappers/dispatcher as `rs/bin/herdr-subagents` plus the lib lifecycle operations they call (bring-up/down/gc/status/read/send/reset/backend/inbox/migrate/keeper/turn), which need the presentation backends.

Tests: 62 foreign tests total (2 new: end-to-end fake-Codex turn in an isolated child test process — the daemon's subreaper/reaping must not share the test binary — and invalid-request quarantine). Run: `cargo test -p agentctl foreign`, `cargo clippy -p agentctl --all-targets`, `cargo fmt --check`.

## 16. B3-7c scope, first slice (headless CLI)

New `foreign::cli` dispatcher + `rs/bin/herdr-subagents` binary (`herdr_subagents_main.rs`), registered in the crate packaging, components, and editions (Rust edition is an explicit `HERDR_SUBAGENTS_ENGINE` opt-in; unset keeps the Python primary, the same pattern as wrkviz). Ported headless commands: `backend` (selected/set with env > config > detection resolution; a dynamic consumer policy module refuses by name instead of being silently ignored), `send`, `read` (headless modes with the exact TUI refusal codes), `reset`, plus `--version`, `--userguide` (the shared embedded user guide, which the foreign guide symlinks to), top-level and per-command help byte-identical to the Python dispatcher, and the hidden `__runner` entry the presentation wrapper will launch. `up`, `status`, `down`, `inbox`, `migrate`, `turn`, `keeper`, and `mcp` refuse by name (exit 2) until their slices: they need the tmux/Herdr presentation launch paths, TUI delivery/read, and the keeper trajectory store. Backend/mode/harness resolution (`selected_backend`, `selected_mode`, `require_supported_harness`) and send/read/reset operations live in `registry.rs`/`lifecycle.rs` for the remaining commands to reuse.

Tests: 63 foreign tests total (1 new CLI test driving backend/send/read/reset against a fake base through the dispatcher). Packaging: `check_rust_packages --crate agentctl` passes with the third binary; embedded-guide check passes. Run: `cargo test -p agentctl foreign`, `cargo clippy -p agentctl --all-targets`, `cargo fmt --check`, `python3 scripts/check_rust_packages.py --crate agentctl`.

## 17. B3-7c scope, second slice (status, down, gc)

Adds `status` and `down` (including `--all-dead` gc and `--force`) to the dispatcher, backed by new lifecycle operations: `archive_state_at`, `recorded_liveness_at` (raw-row proofs in Python's order: registry runner bound by its boot sidecar, a differing runner.pid, harness leader + tree, staged runners; unreadable evidence is inconclusive, never dead), `gc_at` (Herdr workspace-loss snapshot/reap, TUI probe preservation, headless reap only on a Dead verdict after harness termination), `status_snapshot_at`/`AgentStatus` (headless exact; TUI status from the pane probe without the shared-sessions status override), `bring_down_agent_at` (STOP marker, grace poll, terminate, staged-runner stop, liveness refusal unless forced, window kill, archive, registry removal, event), tmux/Herdr window probes, and a bounded Herdr API call helper. Still refusing by name: `up`, `inbox`, `migrate`, `turn`, `keeper`, `mcp` — bring-up/presentation launch, TUI delivery, migration, and the keeper store are the remaining B3-7c slices.

## 18. B3-7c scope, third slice (turn + inbox snapshot)

Adds `turn` for existing agents (fresh-context reset with the return-4 refusal, effort env, send, sentinel streaming wait with pre-dispatch transcript offset, exit codes 0/2/3/4; creation and window recreation still refuse/warn until bring-up lands) and `inbox --inspect` (`tui_inbox_snapshot_at`); `inbox --drain` still refuses by name pending TUI delivery. Also `status`/`down`/`gc` from the second slice are covered there. Remaining B3-7c: `up` (presentation launch: tmux/Herdr windows, runner wrapper, TUI launch), `migrate`, `keeper`, and TUI delivery/read paths.

## 20. B3-7c scope, fifth slice (TUI delivery and read)

Wires TUI workers to the shared Rust transport (`crate::agent::drain` on a direct Herdr client bound by pane, session, harness, workspace, and cwd): send enqueues then delivers (model-override messages quarantined first with attempt records), `inbox --drain` runs the same delivery and reports the resulting snapshot, TUI reads serve bounded recent scrollback (`all`, and `tail` with the 500-line default), and `require_live_agent` now probes TUI panes with the exact missing-pane/probe-failed/dead refusals. The composer clear still refuses, as upstream: Herdr has no draft-state API, so `--clear-composer` can never prove clearing is safe. Remaining B3-7c: `up` TUI launch (visible Codex startup-prompt handling), `migrate` (+ window recreation), and `keeper`.

## 21. B3-7c scope, sixth slice (migrate, headless)

Adds `migrate_agent_at` (backend swap with pause-ack staging, activation commit under the registry lock, source cleanup only after destination commit, full rollback with record restoration and intake resume, plus the legacy pre-handshake tmux path with restart-on-failure) and the `migrate` CLI command. Destination TUI conversion and `recreate_window` remain with the TUI launch slice; `keeper` is the last B3-7c command.

## 19. B3-7c scope, fourth slice (bring-up, headless)

Adds `up` for headless workers: policy-mode resolution, Antigravity preflight, orphan presentation refusal (tmux window / Herdr tab label in the configured workspace), registry row creation, tmux session/window and Herdr workspace/tab launch running the runner wrapper (`herdr-subagents __runner`, stderr capture, keep-alive shell), launch-failure row rollback, first-brief enqueue, and the `agent_up` event. TUI mode bring-up still refuses by name (`tui_not_yet_in_rust`) until the visible-Codex launch slice; dynamic consumer policy modules refuse instead of being silently skipped. Remaining: `migrate`, `keeper`, TUI launch/delivery/read, and window recreation.

## 22. B3-7c scope, seventh slice (TUI launch and window recreation)

Adds `launch_herdr_tui_at` (visible Codex TUI launch in a Herdr workspace/tab, startup-prompt handling, readiness wait) and `recreate_window_at` (tab/tailer recreation for a live agent whose window is gone). TUI reads honour `SUBAGENTS_TUI_READ_LINES` (default 500) via `tui_read_lines`.

## 23. B3-7c scope, eighth slice (migrate TUI conversion)

Completes the destination-TUI branch of `migrate_agent_at`: `convert_headless_to_tui_at` and `convert_legacy_tmux_headless_to_tui_at` stop the recorded runner by pinned identity, launch the TUI (`launch_herdr_tui_at`), wait for the prompt (`wait_for_tui_prompt_at`), submit the staged text (`submit_tui_text_at`), and confirm the resumed session (`confirm_resumed_tui_at`); failure restores the headless runner (`restore_headless_runner_at`) and resumes intake. `turn` now calls `recreate_window_at` in its heal path, warning with the underlying error instead of the not-yet-ported refusal. Remaining B3-7c: `keeper` only (`mcp` is B3-8). Conversion paths are compile- and suite-verified; live-Herdr exercise is still pending.

## 24. B3-7c scope, ninth slice (keeper)

Adds `keeper_ensure_at` / `keeper_retire_at` / `keeper_status_line_at` and the `keeper` CLI command: the workspace is resolved by label WITHOUT creating one, the keeper tab is `subagents-status`, ensure starts the `status` self-invocation loop (`|| true`, `--interval` default 300 s) on the created pane, retire refuses while any agent is registered unless `--force`. Help and argparse error wording differentially identical to Python (help, missing action, invalid action, invalid interval). This completes the B3-7c command surface; only `mcp` (B3-8) still refuses by name in the Rust dispatcher. Live-Herdr exercise of ensure/retire is still pending.

## 25. B3-8 scope, first slice (foreign MCP stdio + WebSocket broadcaster)

Adds `rs/agentctl/src/foreign/mcp.rs` and removes the last `mcp` refusal in the Rust dispatcher. The stdio server speaks newline-delimited JSON-RPC (`initialize` returning protocol `2025-06-18` and server `herdr-foreign-subagents`, `ping`, `tools/list`, `tools/call`; notifications starting `notifications/` get no reply; error codes -32700/-32600/-32601/-32602 match). The nine `subagent_*` tools dispatch onto the existing lifecycle entry points with the established argument validation (`bad_argument` naming the key and expected kind, read mode defaulting to `all`, down on an unknown agent raising `unknown_agent`, unknown tools raising `unknown_tool`); successes carry `structuredContent` plus `ok: true`, failures are `isError` results carrying the structured code and message, and the `content` text uses the established sorted-key wire style so parsed replies are identical between editions (verified by a 13-request differential probe against the Python server, covering initialize/list/ping/list/status-bad-argument/unknown-tool/unknown-agent/missing-method/missing-name/bad-integer/bad-boolean). The `tools/list` schemas are verbatim. The WebSocket broadcaster (default port 18765 from `SUBAGENTS_MCP_WS_PORT`, 0 disables) implements the upgrade accept with the `sha1` crate (new dependency; `base64` was already in the lockfile) plus `base64`, republishes event-log lines and transcript `TURN-DONE` markers with the established deduplication keys and text-frame encoding. `rs/agentctl/tests/herdr_subagents_mcp.rs` drives the real binary over stdio and performs a real WebSocket handshake plus one broadcast event. Adding `sha1` changes `Cargo.lock`, so this slice's gate runs the full exclusive phase with no waiver.
