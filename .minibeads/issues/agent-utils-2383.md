---
title: 'private-run-tmpdir: validate and gate give every run one private, short TMPDIR outside the host temp dir, removed at run end; with-private-tmp for focused runs'
status: open
priority: 2
issue_type: task
assignee: cl-tmpfix
labels:
- validation
- gate
- tests
- tmp
- cleanup
created_at: 2026-11-27T09:49:12-05:00
updated_at: 2026-11-27T09:49:12-05:00
---

# Description

The #2382 summary-link-test-host-tmp audit found that `scripts/validate.py`, `scripts/gate.py` and dagrun give no step a private `TMPDIR`.

- Every gated step inherits the caller's `TMPDIR`, which is usually unset, so 33 gate-script sites and 810 test sites in 438 files can write to the host's `/tmp`. Every Playwright walk's Chromium profile goes there too.
- validate's own scratch (the pytest root of #941, the global gitconfig, the agentctl registry, the NO_RESULT file, cross coverage, case timings) is made with `mkdtemp` in the host temp dir as well.
- dagrun ends a timed-out step with `cgroup.kill` (SIGKILL), so in-test cleanup cannot be relied on. Leftovers pile up in the host temp dir: thousands of entries from tests were found there on 2026-11-27.
- Python's `tempfile` falls back to `/var/tmp` when `/tmp` is not writable. That is no escape: on at least one host, `/var/tmp` is the same volume.
- Only `rust.agentctl.test` has its own scratch: a per-step `/dev/shm` directory (#869).

Fixing each site is hundreds of edits. One run-wide root fixes every gated site at once.

# Design

- validate exports `TMPDIR`, `TMP` and `TEMP` for the whole graph. They point at one private root per run, under a short per-user base on disk outside the host temp dir. gate.py's own scratch moves under the same root.
  - The root is removed when the run ends: passed, failed or SIGTERM.
  - A later run sweeps a root that a SIGKILL left behind, once it is this user's, untouched for 24 hours, and its PID is gone (the #941 rules).
  - The pytest root and validate's other scratch go in the same root.
- The base must meet five constraints:
  - Short: a Unix socket path is limited to 108 bytes, and tests bind sockets under `TMPDIR` and pytest's `tmp_path` (#447, #869).
  - On disk, not tmpfs: tmpfs pages count against step memory caps (#447).
  - Not synced between hosts.
  - Not inside any Git working tree: tests discover Git upward from their temp dirs.
  - Configurable through an environment variable.
  - The length budget is measured on the socket-using suites before the switch.
- A root an outer run already made (gate → validate, or a wrapper → validate) is reused through an environment marker, not nested again.
- Removal never descends into a mount point under the root. A mount a test left is reported and kept, as the sharder already does.
- A step that sets its own `TMPDIR` keeps it: `rust.agentctl.test`'s `/dev/shm` directory. So does a test that pins its own location, such as the socket probe's runtime directory (#1230).
- `scripts/with-private-tmp CMD...` does the same for one command outside a gate, for focused runs: make a root, export it, run, remove it, keep the exit status, and forward SIGTERM.
- Nothing changes in dagrun's containment: per-step cgroups, CPU and memory caps, and `cgroup.kill` stay as they are. `TMPDIR` is only an environment value.

# Acceptance Criteria

- A gate run (passed, failed or SIGTERMed) leaves no entry from its steps in the host temp dir, and leaves no root under the base.
- A root left by a SIGKILLed run is removed by the next run once stale. A live run's root is never touched.
- Every socket-using suite passes under the chosen base, with the measured headroom recorded.
- `rust.agentctl.test` still runs on its own `/dev/shm` scratch.
- `scripts/with-private-tmp` runs a command in a private root and removes it on success, failure and SIGTERM.
- The gate's CPU and memory containment is unchanged.

# Notes

Filed 2026-11-27 9:49 AM ET at the lead coordinator's request, approved as the follow-up to #2382.

Covered here, not fixed one by one: three agentctl library tests leave entries even after a passing run. All three use `std::env::temp_dir()`. In a gate they run inside `rust.agentctl.test`, whose per-step `/dev/shm` scratch already goes when the step exits. Outside a gate, `scripts/with-private-tmp` covers them.

- `rs/agentctl/src/chat_history.rs:887` `tempfile_dir()`: it removes `agentctl-chat-history-test-<pid>` before use but never after.
- `rs/agentctl/src/chat_event_sink.rs:1105`: the inline `temp_dir("carry-fresh3")` is missing from the cleanup loop.
- `rs/agentctl/src/chat_runtime.rs:39655`: the first `temporary("receipt-check-mark-ack")` root is shadowed by the `-old` root at 39666 and never removed.
