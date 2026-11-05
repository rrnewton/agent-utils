---
title: 'agentctl-test-fsync-churn: run the agentctl durability tests on a memory-backed scratch directory'
status: closed
priority: 2
issue_type: bug
assignee: cl-start
created_at: 2026-11-05T04:31:59.492939592+00:00
updated_at: 2026-11-05T08:00:00.000000000+00:00
---

# Description

The `rust.agentctl.test` gate step writes about 11 GiB per minute to the host disk while the chat runtime / chat service durability tests run.

- One agentctl test binary measured 18.4 GiB of `write_bytes` in 110 s.
- Across the whole dagrun slice, a gate wrote 5.1 GiB in one 10 s sample.

The data written is tiny: the test directories hold 4,000-9,000 files of under 1 KB each, 22-46 MB in total. The volume is write amplification. Tests such as `completed_explicitly_closed_requests_churn_beyond_population_cap_without_replay` (`temporary("retirement-churn")`), the admission-boundary tests and the chat_service tests do thousands of durable create / fsync / rename cycles. Every fsync on a copy-on-write filesystem (`/tmp` is btrfs here) also writes metadata and log-tree blocks.

Every gate that selects agentctl pays this, and several gates run in parallel. The step makes the gate host's biggest disk writer.

It is also misattributed. When an agent session launches a gate as a child process, the reaped children's I/O is folded into the session process's `/proc/PID/io`, so the agent process looks as if it writes at that rate itself. A consumer team flagged an agent process for exactly this.

# Design

Preferred: give the agentctl test step a memory-backed scratch directory.
- Set `TMPDIR` for `rust.agentctl.test` (and any other step that runs these churn tests) to a per-step directory on tmpfs, for example under `$XDG_RUNTIME_DIR` (or `/dev/shm` when that is unset). Remove it when the step ends.
- Measured footprint is under 100 MB, so tmpfs (charged to the step's memory cap) is fine. Check the cap headroom anyway.
- Make sure `temporary(...)` in the tests honours `TMPDIR` (`std::env::temp_dir()` does).

Do not weaken the durability the tests prove. The fsync calls stay; only the backing store changes. A churn loop may be shortened only if that keeps its population-cap and retirement-sequence assertions meaningful.

# Acceptance Criteria

- One gate's `rust.agentctl.test` step writes under 1 GiB to the disk (measured from the step cgroup's `io.stat` wbytes), down from tens of GiB.
- The test set and its assertions are unchanged.
- Step duration is the same or better.

# Notes

Found on 2026-11-04 while investigating a disk-writer report from the lead. The flagged process's own session file was only being appended to (same inode, about 160 KB per 45 s); the volume came from gate children.

Assigned to cl-start after #852 P0b; reassign to whichever lane frees first.

## Resolution

[opus 5.5] cl-start, 2026-11-05. Landed as d23e7c2c. `rust.agentctl.test` runs with TMPDIR on a per-step tmpfs directory (/dev/shm, else $XDG_RUNTIME_DIR), removed when the step exits; fsyncs and assertions unchanged. Measured with the exact step command in its own systemd scope (dagrun's step cgroups have no io controller, so they have no io.stat): wbytes 194 MiB to the /tmp disk and 199 MiB to the slot's loop disk (build outputs), about 0.4 GiB against about 18 GiB before. All 2126 cases passed in 340 s in the gate. /dev/shm comes first because $XDG_RUNTIME_DIR pushed a headless test's control socket path to the 108-byte limit. The faster disk also exposed a real bug, fixed in 9e57a7d1: a prompt typed to the previous agent in the millisecond of a repoint was reminded to the new agent.
