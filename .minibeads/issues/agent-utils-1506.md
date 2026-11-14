---
title: 'rust-client-proc-readers-under-load: client proc readers mis-read vanishing records under load'
status: closed
priority: 1
issue_type: bug
created_at: 2026-11-14T02:08:52.677801778+00:00
updated_at: 2026-11-14T03:49:34.518109213+00:00
closed_at: 2026-11-14T03:49:34.518108812+00:00
---

# Description

Two failures seen under host load in rust.agentctl.test, both in rs/agentctl/src/client.rs:

1. client::tests::real_supported_shell_with_background_descendant_is_not_idle: pane_is_idle_shell said idle while a background sleep the test had just seen still existed. The descendant scan (process_has_no_descendants / idle_shell_proof) can mis-read /proc under load: a child's stat or children file vanishing, or racing, gets treated as no descendants. The test also waits on a 5 s wall-clock deadline, which owner rule 346 forbids: wait on the condition with a generous timeout and fail with diagnostics instead.

2. client::tests::recorded_pane_shell_identity_refuses_every_generation_change: unwrap on AdapterError cannot parse process stat for pid N (kind Unavailable): client.rs's own /proc/PID/stat read racing a process exit.

#1495 already fixed the same race class in rs/agent-queue identity, rs/agentctl foreign ownership, turn_runner and py client. Reuse its classification for EVERY /proc reader in client.rs.

# Design

Reuse the #1495 classification at the read (unopenable ENOENT/ESRCH, empty, or vanished-on-re-read => process gone; a stably readable malformed record still refuses) for every /proc reader in rs/agentctl/src/client.rs (stat, children, task, cgroup, cmdline, exe readers), choosing per call site what gone means: for idle-shell, a descendant whose record vanished mid-scan was alive at the scan's start, so the shell is NOT idle (fail safe, Ok(false), not an error); for identity, gone means a generation change (Ok(false)/None, not a parse error). Make the Python edition (py/agentctl/client.py twins) agree. Replace the 5 s wall-clock wait in the named test with a condition wait with generous timeout and diagnostics. Fixture proc roots make each reader testable without real /proc races.

# Acceptance Criteria

- One unit case per shape per reader (empty, truncated, vanished-on-re-read stat; children listing racing; stable malformed still refuses) in the Rust edition, with fixture proc roots and no wall-clock waits.
- The Python twins classify identically, with a unit case per shape.
- Both named tests pass robustly at >75% CPU on many threads, with no wall-clock deadline in the descendant wait.
- Fresh cpuset local validate passes, then a 3-slot gate passes; a red in client.rs is touched, not waivable.

# Notes

Filed by the mu-clientproc lane from coordinator gate evidence under host load.
## Resolution

[watermelon-full-preview] Confirmed by reading every `/proc` reader in
`rs/agentctl/src/client.rs` and by reproducing
`real_supported_shell_with_background_descendant_is_not_idle` solo:
the descendant proof's parent-link fallback read each process's stat
with `fs::read` and propagated `parse_process_stat` failures, so a
record that was empty, torn, or vanished mid-scan under load surfaced
as `cannot parse process stat for pid N` (an `AdapterError`, the
restore-drill refusal) or skipped a live descendant, and the proof
could call a shell with a live background child idle. The test itself
also waited on a 5 s wall-clock deadline (owner rule 346) against a
30 s fixture sleep while each stable-identity observation takes
seconds under load. `process_children_map` read stats through
`bounded_file`, which turned the same vanishing shapes into skips
without the #1495 classification.

Fix: every client.rs procfs reader now classifies at the read under
the #1495 rule via `proc_stat_parent` /
`proc_stat_parent_with` / `read_stat_record` (`ProcStatParent`):
unopenable (ENOENT/ESRCH), empty, or vanished-on-re-read is `Gone`
(the process exited); only a record that stays readable and
unparseable is `Malformed`. Per call site: the idle-shell descendant
proof (`process_has_no_descendants_in`, now fixture-rooted) treats
`Malformed` as inconclusive — `false`, never idle and never an error —
skips `Gone` records as exited processes, and requires two
consecutive complete parent-link scans under a stable task set when
the per-task `children` listing is unavailable or vanished; identity
callers see gone as a generation change, not a parse refusal. The
children-map reader classifies the same way and skips gone records.
The `cgroup`, `exe`-link, and `cmdline` readers already mapped any
read failure to the safe negative in both editions and are unchanged.
The Python twin (`py/agentctl/client.py`
`_process_has_no_descendants`, now with an injectable proc root)
classifies identically. The named descendant test now waits on the
condition with a generous bound and diagnostics, against a 300 s
fixture sleep. One case per shape per reader in each edition:
scripted-reader, fixture-proc-root, and children-listing cases, none
with wall-clock waits.

Verified: both named tests and every new fixture case pass in both
editions; a fresh cpuset local validate passes (`validate exit: 0`),
then a 3-slot gate passes (`gate exit: 0`).
