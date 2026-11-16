---
title: 'agentctl-revive-dryrun-identity-poll-hang: dry-run revive polls a shell identity instead of answering'
status: closed
priority: 1
issue_type: bug
labels:
- agentctl
created_at: 2026-11-16T20:49:34+00:00
updated_at: 2026-11-17T14:20:00+00:00
closed_at: 2026-11-17T14:20:00+00:00
---

# Description

[watermelon-full-preview] Evidence from another host (agentctl f13aabf3, herdr backend): plain `revive NAME --dry-run` hangs only against a live record whose pane is gone (the state a timed-out `--archived` revive leaves). Control: `revive NAME --archived --dry-run` with no live record returns its plan instantly. While hanging: zero output; wchan `hrtimer_nanosleep`; `lslocks` empty; no children. strace, 10 s: a tight repeating loop of openat/statx/readlink on `/proc/<pid>/exe` and `/proc/<pid>/stat` plus `/proc/sys/kernel/random/boot_id`, where `<pid>` is a bash. So a process-identity poll that never converges: it sleeps and re-checks instead of answering.

# Design

Give the poll a terminal answer. A live-but-not-the-harness pid, or a pane that is gone, must yield a plan that says so, never a wait. Any wait that remains needs an explicit bound that reports, not a silent loop. The poll sits in the identity/shell proofs the dry-run proof walks (the recorded pid's liveness and the idle-shell census), which re-observe a shell's `/proc` identity at full timeout per look and scan candidates in an order that pays that cost before the cheap filters.

# Acceptance Criteria

- `revive NAME --dry-run` against a live record whose pane is gone returns a plan naming that state, without an unbounded identity poll.
- Any remaining identity wait is explicitly bounded and reports when the bound ends.
- A regression test with a stub process table; condition-based, with no wall-clock budget in the assertion.

# Notes

## Resolution

[watermelon-full-preview] The poll was the missing-pane orphan scan in revive's proof: it proved every census pane's idle-shell identity — each re-observing a bash's `/proc` identity, at the caller's full 5 s timeout per look because a shell's final image is its interpreter, which `observe_stable` never accepted early — before the cheap cwd/label/ownership filters. Two fixes: `observe_stable` accepts an interpreter after one second of unchanged identity (`identity_accepted` in agent-queue's identity module; final harness images keep the 200 ms window, the caller's deadline remains the bound), and the orphan scan runs cwd/label/ownership filters before proving any shell. Regression: `revive_dry_run_with_a_gone_pane_never_proves_unrelated_shells` (stub census counts shell proofs; no wall-clock assertion) and the pure stable-window decision test. Landed on main; see the landing commit for the SHA.
