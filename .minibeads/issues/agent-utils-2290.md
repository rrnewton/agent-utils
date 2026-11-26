---
title: 'agentctl-remaining-cmdline-in-output: the work-state lock-holder clause and kill-pid name processes by pid, comm and action, never by command line'
status: open
priority: 2
issue_type: bug
labels:
- agentctl
- security
created_at: 2026-11-26T07:48:00-05:00
updated_at: 2026-11-26T07:48:00-05:00
---

# Description

#2217 stopped lock-contention reports, waiter records, the migrate-layout in-use reports and the chat health runner from carrying full process command lines. Its audit of `command_line(` callers found two other readers of `/proc/PID/cmdline` that still put a command line into output:

1. The work-state report (`rs/agentctl/src/work_state.rs`, `waits_on`) explains why an agent's tool is asleep. It names the waiting process and the process holding the file lock it waits on as `` `<command line>` (PID n) ``, bounded to 80 characters. The holder comes from `/proc/locks` and can be any process on the host, not just the agent's own. The clause reaches watch notices through `agent_watch.rs`.
2. `agentctl kill-pid` (Rust `kill_pid.rs`, Python `killpid.py`) matches `--expect-cmdline` as a substring of the target's command line. Its `cmdline-mismatch` refusal prints the target's full command line, and so do its success report (`cli.rs`, `cli.py`) and its audit log line (`kill-pid.jsonl` in the registry). A wrong or reused pid therefore prints another process's full argv to the caller.

Command lines can hold private paths, brief file names and, in other setups, credentials passed as arguments; see #2217 for the rule.

# Design

- Work state: name any process outside the agent's own process tree (the lock holder in particular) by `pid`, `comm` and, for an agentctl process, its derived subcommand and target agent, using #2217's `lock_wait` helpers. The agent's own tool and step commands may keep their bounded label, since they are that agent's own commands, already visible in its transcript; if that is changed too, say so in the resolution.
- kill-pid: matching may keep using argv. It is part of the identity check, together with the start time, against a reused pid. Printing argv should stop. The mismatch refusal says that the expected text was not found, and names the process by `pid` and `comm`. The success report and the audit log record `pid`, `comm`, `starttime`, `cwd` and a digest of the matched command line (enough to tell two runs apart), not the command line itself. If some field of the argv is truly needed in the log, justify it in the resolution and record only that field.
- Rust and Python editions change together, as #2217 did. The audit log's `cmdline` field goes away. Check readers of `kill-pid.jsonl` and treat it as a persisted-field change.

# Acceptance Criteria

- Test: a work-state report where an agent's tool waits on a file lock held by a process outside its tree names the holder by pid and comm; a sentinel argument in the holder's argv is absent from the report and from the watch notice built from it.
- Test (Rust and Python): a kill-pid `cmdline-mismatch` refusal against a process carrying a sentinel argument does not contain the sentinel; neither do the success report or the audit log line of a matching kill.
- The persisted-field change to `kill-pid.jsonl` is listed in the resolution.

# Notes

Follow-up of #2217 (its resolution records this audit).
