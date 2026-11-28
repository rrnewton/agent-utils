---
title: 'socket-selftest-cross-namespace-sweep: the socket-probe self-test sweeps stale directories by judging each creator PID in its own PID namespace, so in a namespaced test shard it can remove a live self-test directory of another gate'
status: open
priority: 1
issue_type: bug
assignee: cl-flaketriage
labels:
- wrkslots
- census
- tests
created_at: 2026-11-28T10:28:00-05:00
updated_at: 2026-11-28T10:28:00-05:00
---

# Description

The Python socket-probe self-test (#1230) works in a directory under a host-wide parent. Before it starts, it sweeps that parent for "stale" self-test directories. The sweep decides whether a creator is still running by looking up its PID in the sweeping process's own PID namespace.
- Parent: `_socket_self_test_parent` (`py/wrkslots/cli.py:97312` on 2026-11-28 main) returns `/run/user/<uid>` when that is a real directory owned by the caller, else `/tmp`.
- Directory: `wrkslots-probe-selftest.<pid>.<start ticks>.<random tag>`, made by `_run_socket_probe_self_test` (`cli.py:97360`). It first sweeps the parent (`cli.py:97378`).
- Sweep: `_stale_socket_directories` (`cli.py:96601`) treats a directory as stale when it meets all of these:
  - it is owned by the caller's uid;
  - `/proc/<pid>` shows no process with the recorded start ticks;
  - it holds nothing but the socket node `s`.
  `_remove_stale_socket_directories` then unlinks `s` and removes the directory.

Since #2065, the wrkslots test shards run in their own user and PID namespaces:
- Every shard's pytest is the same small pid inside its namespace (20 in a gate).
- uid 0 is mapped there, so `/run/user/0` is missing and the parent is the host `/tmp`, shared by every gate.
- Another gate's self-test directory carries a pid from that gate's namespace. Seen from this namespace, the pid is absent, or belongs to another process with different start ticks. So the sweep calls the directory stale and removes it, possibly while that gate's self-test is between bind and connect.
- The names themselves are unique (random tag), so the hazard is the sweep, not a name clash.
- Not yet seen in a gate. Found by the #2065 sweep for host-wide paths a namespaced shard can share with another gate. That sweep also found the `/dev/shm` fixture collision fixed under #2065.

The calibration directories `.wrkslots-calib.<pid>.<start ticks>.<random tag>` have the same judgment:
- made at `cli.py:97177`, swept at `cli.py:97568`, `cli.py:97620` and `cli.py:102793`;
- they live inside the census targets, which tests keep private (per-test directories, including `/dev/shm`), so they are safe in tests today;
- the caveat holds wherever two PID namespaces share a target.

The Rust edition has no socket-probe self-test or calibration directory.

# Design

- Record the creator's PID namespace with each self-test and calibration directory: in the name, next to pid and start ticks, as the inode of `/proc/self/ns/pid`.
- The sweep judges only directories from its own PID namespace, by pid and start ticks as today. It leaves every other namespace's directories alone; those are never stale from here. `scripts/private_tmp.py` owner records already do this (their `pid_namespace` field, `_pid_namespace`).
- A directory in the old name format (no namespace) is judged as today. It has the same reach as before the change; it can be dropped once no writer makes the old format.
- No persisted wrkslots field: these are scratch directories, never state.
- Tests, deterministic, on a fixture parent:
  - a directory whose recorded namespace is not the sweeper's, with a dead-looking pid, is kept;
  - one from the sweeper's own namespace whose creator is gone is removed;
  - one whose creator is live is kept.
- No wall-clock budget (owner rule 346).
- To audit: the product readers of the real gate admission directory, `_gate_admission_dir` (`cli.py:60766`). It uses `$GATE_ADMISSION_DIR`, else the legacy `/tmp/dev-agent-utils-gate-admission`, else `~/.local/state/agent-utils/gate-admission`; its reader is at `cli.py:60831`. Check whether any namespaced test reaches it without `GATE_ADMISSION_DIR` set, and whether it judges host PIDs from inside a namespace.

# Acceptance Criteria

- The self-test and calibration sweeps never remove a directory made in another PID namespace; their stale judgment covers only the sweeper's own namespace.
- The fixture tests above pass.
- The gate-admission-directory readers are audited for namespaced tests, with each reader named and found safe or fixed.
- No new persisted wrkslots field; no wall-clock budget.

# Notes

Filed by `cl-flaketriage` at the coordinator's request (2026-11-28; number pre-assigned), from the #2065 sweep for host-wide paths shared across namespaced shards.

Related:
- #2065: the namespaced wrkslots shards, and the `/dev/shm` fixture collision fixed under it.
- #1230: the socket probe and its self-test.
- #2383: the gate's private temp root (it covers TMPDIR, not `/tmp` itself or `/dev/shm`).
