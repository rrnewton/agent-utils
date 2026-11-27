---
title: 'gate-sigterm-abort-leaves-dagrun-scope: a gate aborted by SIGTERM leaves its dagrun scope and step processes running'
status: open
priority: 2
issue_type: bug
labels:
- gate
- dagrun
- validation
created_at: 2026-11-27T13:36:27-05:00
updated_at: 2026-11-27T15:38:17-05:00
---

# Description

[opus 5.5] Aborting a running gate stops gate.py and dagrun, but not the steps dagrun started. They keep running in dagrun's scope, outside any gate slot, until someone stops the scope by hand.

Seen on 2026-11-27 with a full gate (ticket 1795777479530002260-7c2ea464, gate.py pid 2348532), withdrawn while it ran because a descendant tree had already passed:

- SIGTERM to gate.py (its documented abort; gate.py has no cancel subcommand). gate.py logged `gate: aborted by SIGTERM; run's process group stopped by SIGTERM`, recorded rc 143 and exited.
- dagrun itself (pid 3260256) was gone. Its scope, `dagrun-3260256.scope`, stayed active with 44 processes in four step cgroups: `cross.agentctl.differential-0` (15), `cross.agentctl.differential-1` (7), `cross.agentctl.differential-2` (11) and `python.agentctl.shard-0` (11). The steps `python.agentctl.shard-1` and `rust.agentctl.test` were already empty.
- The survivors were still working: a pytest shard, a Herdr test server and the agentctl it drove, tmux test sessions, and fake Muse runtimes from the cross differentials. All were reparented to the user's systemd manager.
- `systemctl --user stop dagrun-3260256.scope` cleared all of them, which left 0 processes in the worktree.

gate.py's abort sends SIGTERM to the run's process group, and calls the abort done when that group is empty. dagrun starts each step in its own session, inside its own step cgroup, so an empty group says nothing about the steps. dagrun installs a SIGTERM handler that runs `cgroup.kill` on the whole scope and stops the unit (`install_scope_teardown`). Here the scope outlived dagrun, so that teardown did not happen or did not finish. The gate log has no "stopping outer scope" line from dagrun. That proves little, because gate.py stops reading the run's output once the abort starts.

# Design

Make an abort stop the run's whole scope or cgroup, not only its process group, and confirm the result before reporting the abort done.

- gate.py learns the run's scope (dagrun's unit and cgroup, published where gate.py can read it, or a cgroup gate.py creates around the run). After SIGTERM and the grace period, it stops that scope or writes `cgroup.kill`. It reports the abort done only once the cgroup has no processes.
- Find out why dagrun's own teardown did not run or did not finish. Candidates to check: the handler was not installed for this run (the in-scope check or the scope unit and cgroup not found), it failed before `cgroup.kill` (for example on writing its message to the run's output), or dagrun died before the handler thread ran. Fix that path too, so a plain SIGTERM to dagrun also leaves nothing behind.
- The ledger row's `aborted` text names the result: scope stopped and empty, or what was left.

# Acceptance Criteria

- An aborted gate leaves no step process and no active dagrun scope or step cgroup, including steps that start their own session or double-fork.
- A hermetic test proves it without a real host gate: a fake run command that starts a dagrun-shaped tree (a step that calls setsid and keeps running) inside a scratch scope or cgroup, then aborts gate.py with SIGTERM and asserts that the cgroup is empty and the unit inactive. Where the host has no user systemd or cgroup delegation, the test skips and says why.
- The abort's log line and ledger row say whether the scope was emptied.

# Notes

- Found while withdrawing a #2359 gate after the #2362 tree, which contains it, passed (2026-11-27).
- Related: #1254 (per-gate CPU containment), #1370 (preflight containment).
- Second instance on 2026-11-27, with SIGINT. A full gate (ticket 1795798933471760095-b80fe1ee) was withdrawn about 3 minutes after admission, because a sibling change on the same files landed first and the tree needed a rebase.
  - SIGINT to gate.py logged `gate: aborted by SIGINT; run's process group stopped by SIGTERM`. gate.py exited with rc 130, and the ledger row says `aborted`.
  - dagrun was gone: the scope's `supervisor` cgroup was empty. `dagrun-2185134.scope` stayed active with 15 processes (`pids.current`) in its step cgroups. `python.agentctl.build-rust` was still running a release `cargo build` and its `rustc`, and `repository.hygiene.issue-parse` and `repository.hygiene.issue-parse-self-test` were building `mb`. One `rustc` started about a minute after the abort.
  - The step processes were in their own process group, not gate.py's.
  - Every process in that one scope had its working directory in the aborted gate's worktree. Writing 1 to the scope's `cgroup.kill` cleared them: the unit went inactive, and no process was left in the worktree.
