---
title: 'agentctl-lock-contention-no-argv: lock contention reports name processes by pid and agent, never by full command line'
status: closed
priority: 1
issue_type: bug
labels:
- agentctl
- security
created_at: 2026-11-24T12:44:00-05:00
updated_at: 2026-11-26T07:45:00-05:00
---

# Description

When `agentctl send` is refused because another process holds the agent's lifecycle lock, the refusal JSON includes a contention report built by `lock_wait::contention`. That report carries `"command"`, the full command line from `/proc/PID/cmdline`, for the lock holder and for every waiting sender. Any caller that hits a busy lock therefore sees other processes' complete argv in its own tool output. The agent watcher's delivery alarms carry the same report.

Command lines can hold paths, file names of private briefs and, in other setups, credentials passed as arguments. Consumers have a standing rule never to print full process command lines, and this output breaks it without the caller asking. Seen by a consumer lane on 2026-11-24; no secrets appeared in that instance.

# Design

- `contention` reports each process as `pid`, `comm` (from `/proc/PID/comm`, 15 bytes) and, where it can be derived without exposing arguments, the agentctl subcommand and target agent name (for example `send liaison`). It never includes the raw command line.
- `command_of` / `command_line` stop being used for any user-visible report. A debug path that truly needs argv must be explicit and opt-in, and must not run by default.
- The same rule applies to every other report that names a foreign process: watch alarms, lock-wait messages, restore and revive refusals. Grep for `command_line(` callers that format into output, and convert them.

# Acceptance Criteria

- Test: a contention report for a held lock with a waiting sender contains pid and comm (plus subcommand and agent where derivable) for both, and no field holds the full command line. The test plants a sentinel argument and asserts it is absent from the JSON.
- Test: a watch delivery alarm built from a contention report contains no command line.
- An audit of `command_line(` callers is recorded in the resolution, with each output-facing one converted.

# Notes

Reported by a consumer team's coordinator on 2026-11-24. Interim mitigation on their side: lanes don't quote that output.

## Resolution

Landed on main in the agentctl stack a724562b..7e89d795 (12 commits replayed by land-main from the gated 62a72cde, the same patch, in the order #2218, #2214, #2222, #2217), as f8b28076, 7cb01697, 3d9190cc, 8a14267b and 7e89d795. Gate ticket 1795671224491121398-107888b7: full selection, 65 steps, pass, finished 7:32 AM ET on 2026-11-26 (rust.agentctl 3106 passed, 0 failed; both python.agentctl shards ran, 1457 passed each). Both python.agentctl shards ran in this gate, so the Python changes have gate evidence too (#2253 did not hide them this time).

- Contention reports, lock-waiter records, the migrate-layout in-use reports (Rust and Python) and the chat health runner now name a process by `pid`, `comm` (`/proc/PID/comm`) and, for an agentctl process, its derived `subcommand` and target `agent` (for example `send liaison`). The raw command line is read only to derive that action and is never reported. A legacy waiter record's stored `command` is never surfaced. The in-use reports and the chat health runner's JSON now carry `comm` where they carried `command`.
- Persisted-field change: the lock-waiter record writes `comm`, `subcommand` and `agent`, and no longer writes `command`. The deployed, pinned agentctl release that consumers read with was tested on 2026-11-26 by the lead coordinator: it accepts a lock-waiter record without `command` and reports `command: null`.
- Tests plant sentinel arguments and assert they are absent from the contention JSON (`a_process_report_names_pid_comm_and_action_never_the_command_line`, `a_legacy_waiter_records_command_line_is_never_reported`, `an_agentctl_action_is_derived_without_its_arguments`) and from the watch delivery alarm sentence (the watch jam test in `agent_watch.rs`).
- 8a14267b fixes three python.agentctl failures met on the way, none caused by the report change: a lock wait with no deadline takes the blocking flock instead of polling every 10 ms, the `_enqueue` test double accepts `deadline_end`, and the live-holder scan in tests sees only the test's own process tree.

Audit of `command_line(` callers in the landed tree:
- Converted (output-facing): `lock_wait::contention` and the waiter records, the `registry_layout` in-use reports (Rust and Python), the chat health runner.
- The remaining `process_command_line` callers use argv only to match identity and never print it: the input refusal's resume check (`subagents.rs`; it looks for the record's own session id in argv, and the message names that session, not argv), the two pane-rebind proofs (`subagents.rs`; argv[0] against the recorded harness, argv against the recorded launch argv), launch-spec capture (`launch_spec.rs`; the agent's own argv into its own record) and `reexecuted_command` (`client.rs`; re-executes a pinned process). Python's callers in `subagents.py` are the same identity checks.
- Outside the `command_line(` callers, two readers of `/proc/PID/cmdline` still put a command line into output and were not converted here: the work-state report (`work_state.rs`) names a process holding a file lock that an agent's tool waits on by its bounded command line, and that holder can be another process; and `kill-pid`'s `cmdline-mismatch` refusal and audit log carry the target's command line (the caller names that pid and an expected command line). Both were reported to the coordinator as follow-up candidates.
