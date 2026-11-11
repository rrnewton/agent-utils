---
title: 'probe-reap-test: a killed probe test counts only its own children, not the whole test process'
status: closed
priority: 0
issue_type: bug
labels:
- merge-queue
created_at: 2026-11-11T15:20:41+00:00
updated_at: 2026-11-11T17:43:53+00:00
closed_at: 2026-11-11T17:43:53+00:00
---

# Description

[watermelon-full-preview] The merge-queue deploy blocker from the originating report (kept at /tmp/coord014-replies/mq-zombie-report.md, 2026-11-11): rs/merge-queue/src/donor.rs test a_killed_probe_leaves_no_zombie_children (added by #1294 at c9ccf970) fails about 1 run in 3 under parallel libtest, and blocked a merge-queue deploy build.

The test's zombie_children() counts EVERY zombie child of the test process via /proc. libtest runs all the crate's tests as threads of one process, and donor.rs alone has 21 Command::new/spawn sites whose children are briefly zombies before their own wait. Any sibling test's transient zombie therefore fails this test, even though the probe under test was reaped correctly.

Evidence (devbig014, c9ccf970, release, from the originating report):

| Run | Result |
|---|---|
| deploy build, full crate suite | FAILED 267/1 (left: 2) |
| --exact, 3 runs | 3/3 pass |
| full suite, default threads, run 1 | 268 pass |
| full suite, default threads, run 2 | FAILED 267/1 (left: 4) |
| --test-threads=1 | 268 pass (93.7 s) |

Impact: the c9ccf970 deploy build failed at 10:10 AM ET on 2026-11-11; the deploy proceeded on a rerun (owner rule 11-05, flake harvested). Promoted to owner tier as a merge-queue bug (owner rule 11-07).

# Design

- Assert on the probe's OWN process only, with no production API change: the probe command records its pid (for example "echo $$ > <work>/probe.pid; exec sleep 30").
- After probe_once returns, assert /proc/<pid> is gone, or at least not in Z state, and that no live process in the probe's process group (pgid) survives, because probe_once kills the whole group.
- Never count process-wide zombies again: remove the zombie_children() process-wide count from this test.
- No timing assertions (owner rule 346): the check reads /proc state after probe_once returns, it does not sleep or budget wall-clock time.

# Acceptance Criteria

- a_killed_probe_leaves_no_zombie_children asserts only on the pid recorded by its own probe command and on that pid's process group, never on every zombie child of the test process.
- cargo test --release --locked -p merge-queue passes the full suite at default threads, 10 runs in a row, on a busy host.
- No production behaviour changes: probe_once, kill_group, and every other non-test path are untouched.

# Notes

Blocks merge-queue deploys. Follows #1294 donor-probe-pacing-bound, which added the test and the process-group kill it verifies. Filed by mu-zombie before implementation; the fix lands separately on mu-zombie/probe-reap-test.

## Resolution

[watermelon-full-preview] Fixed in the test only; no production code changed. The probe command now records its own pid (echo $$ > probe.pid; exec sleep 30), and after probe_once returns the test asserts that pid is gone or not in Z state in /proc, and that no live process remains in its process group. The process-wide zombie_children() count is removed, so sibling tests' transient zombies can no longer fail it. Verified: cargo test --release --locked -p merge-queue, full suite at default threads, 10 runs in a row green on the busy host.
