---
title: 'python-wrapper-post-fork-hang: on hosts where python3 is a launcher that runs the interpreter as its child, the launcher can hang after the child exits, at zero CPU with SIGTERM blocked, holding a gate step until its wall timeout'
status: open
priority: 1
issue_type: bug
assignee: cl-flaketriage
labels:
- validation
- gate
- hot-host
created_at: 2026-11-28T14:14:00-05:00
updated_at: 2026-11-28T14:41:00-05:00
---

# Description

On some hosts `python3` is not the interpreter itself but a launcher.
- `python3 X`, or a script whose first line is `#!/usr/bin/env python3`, starts a launcher process. The launcher runs the real interpreter as its child and stays alive as that child's parent for the whole run.
- It installs handlers for almost every signal, to forward them to the child.
- Its allocator runs a jemalloc background thread.
- The child interpreter has neither the catch-all handlers nor the allocator thread. It catches only what the program asks for.

Twice on 2026-11-28 such a launcher was seen hung after its child had exited:
- the launcher of `scripts/with-private-tmp --host-temp ...`, at load1 above 230. The wrapped command (a namespaced pytest) had passed, and the with-private-tmp interpreter had exited.
- the launcher of an operator's own loop script.

Both had the same signature:
- the launcher's main thread blocked in `__futex_wait`;
- its `jemalloc_bg_thd` thread also in `__futex_wait`;
- its child, the finished interpreter, a zombie, never reaped;
- SIGTERM and SIGCHLD pending but blocked (`SigBlk` 0x24000), so a `timeout` around the command could not end it. Only SIGKILL did.

The program's own code had already returned. The hang is in the launcher, after or while its child exits. A fix inside a wrapper's Python code cannot reach it: the hung process is that code's parent.

Why it matters: every `python3 ...` command in the validation DAG, and every `#!/usr/bin/env python3` script it runs (`scripts/with-private-tmp`, `scripts/case_timings.py`, the component runners, gate.py itself), has such a launcher parent on these hosts. A launcher hung this way:
- holds its step until the step's wall timeout, often hours;
- uses about 0 CPU, so the CPU timeout never fires;
- ignores SIGTERM.

The rate is low:
- No hang in 460 repro runs on 2026-11-28: 300 short wrapper runs and 160 in the observed shape.
- The one natural hang came in about 110 runs.
- dagrun's "wall-stalled cpu=0.0Xs" reports in recent kept logs are something else: wrappers waiting on live children, in steps that hit their CPU timeout.

# Design

- (a) Repro: a large background run in the observed shape. Capture a hung launcher by exact PID: its native stacks (gdb `thread apply all bt`, py-spy `--native`), `/proc/<pid>/status` (SigBlk, SigPnd, ShdPnd, SigCgt) and its threads.
- (b) Survey every DAG step and wrapper that starts through `python3 ...` or `#!/usr/bin/env python3`, and mark which are long-lived (alive for a whole step).
- (c) Mitigate in code: long-lived DAG wrapper commands run the resolved interpreter directly (`os.path.realpath(sys.executable)`, resolved once), so no launcher parent lives for a whole step.
  - The namespaced lifecycle steps already do this for their inner pytest.
  - First cut: with-private-tmp's own child, and `case_timings.py`. Broader edits only after that.
  - Portable: on a host where `python3` is the interpreter itself, the resolved path is the same binary, and behaviour is unchanged.
- (d) Evidence, since the natural rate is about 1 in 100+:
  - the mechanism: a process tree during a step, before and after, showing no launcher parent alive for the step;
  - a large-N run in the observed shape, with load1 recorded per run.
- No wall-clock budget (owner rule 346).

# Acceptance Criteria

- A hung launcher's stacks are captured and named, or the hang stays unreproduced after a large N and the mechanism evidence stands alone.
- The long-lived wrappers in the first cut run with no launcher parent during a step, shown by a process tree, and behave the same where `python3` is the interpreter.
- A large-N run in the observed shape shows no hang after the change.
- The gate stays green.

# Notes

Filed by `cl-flaketriage` at the coordinator's request (2026-11-28; number pre-assigned), from the #2488 churn repro. Corrected the same day: the hung process is the interpreter launcher, not with-private-tmp's own code. The system python (no launcher, no allocator thread) has not hung in this lane's loops since they moved to it.

Related:
- #2383: the private temp root that `with-private-tmp` sets up.
- #2472: a stopped gate leaves its dagrun scope running (a separate defect in the stop path).
