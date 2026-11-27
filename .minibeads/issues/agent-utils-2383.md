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

## Socket path measurement (before any gate)

[opus 5.5] cl-tmpfix, 2026-11-27, load1 110-178. Each entry is the longest AF_UNIX path a suite bound or connected to. Python paths were logged by a `socket` hook in every Python process, Rust and Herdr paths by `strace -e bind,connect`. The maximum usable length is 107 bytes.

- Candidate root `~/.cache/agent-utils/tmp/v<pid>-XXXX` (49 characters), plain pytest: 222 `AF_UNIX path too long` failures in the agentctl chat and socket suites, plus 4 in the restore drill ("local socket name length exceeds capacity"). Rejected.
- agentctl headless Rust library tests (measured by a sibling lane on its existing build):
  - 9 failed at a 46-character `TMPDIR` and passed at 35.
  - At 26 characters all 9 passed, and the longest path was 93 bytes, so 14 were left (`<root>/agentctl-wrap-console-refused-<pid>-<n>/reg/agent/control.sock`).
  - In a gate they run under `rust.agentctl.test`'s own `/dev/shm` scratch, which this change leaves as it is.
- Chosen root `~/.aut/<pid>` (25 characters, 26 with a clash letter). Every suite below ran in the gate's own layout (`run_component_tests.py` shards and the sharder):
  - Python agentctl component, both shards, 2917 tests: all passed. The longest path was 93 bytes (`<root>/s0<4 hex>/test_existing_over_cap_inbound0/disabled-over-cap/.wake.sock`), so 14 were left. The same test in today's `/tmp` layout is 103 bytes, leaving 4.
  - The sharder's basetemp inside a private root is `<root>/s<shard><4 hex>`. The long `agent-utils-pytest-<pid>-<shard>-<random>` name is kept for a shared temp dir, where its sweep needs it.
  - Restore drill (`$HOME/.config/herdr/sessions/<session>/herdr-client.sock` under a short scratch HOME): about 105 bytes today, leaving 2. With the HOME prefix `h` and the session name `d<8 hex>` it is about 87 bytes, leaving 20, and all 4 drill tests pass.
  - wrkslots socket tests (`test_proc_status`, `test_direct_census_found_evidence`) and the archive fetcher: 218 passed.
- chat-subscription-slack warm sockets are `<root>/slack-warm-<name>-<pid>-<n>/w.sock`, about 85 bytes. agent-teams prefers `$XDG_RUNTIME_DIR`.

## Findings along the way

- The restore drill's `test_restore_drill_missing_slot_bind_refuses_only_that_lane` mounts slot images with fuse2fs (user-level `fuse.ext4`, no root).
  - In one run under strace, its own `fusermount -u` failed, and two mounts and their fuse2fs daemons were left behind. The mount guard kept the root, as designed, instead of removing through the mounts. The leftovers were released with a user-level `fusermount -u` and a SIGTERM to each daemon.
  - One run without strace: all 4 passed, with no mounts left. The daemons exited on their own within minutes.
  - So the leak came from strace, not the change. Hardening (not done here): the test should unmount in a `finally` block and verify that the mount is gone.
- On at least one host, home is also visible through a bind mount at a second path, so every mount under `~` appears twice in `/proc/self/mountinfo`. The mount guard matches by the root's path as given and as resolved, and an unmount by the `~` path works.
- A test that copies `scripts/gate.py` into a scratch checkout must now copy `scripts/private_tmp.py` beside it.

## The first gate and the scoped host-temp exception

[opus 5.5] cl-tmpfix, 2026-11-27. The first full gate (3d421bfd, ticket 1795811325223360050-2994da33) failed 19 steps.

- 14 came from this change. On the gating host, `~/.aut` and `/tmp` are different btrfs subvolumes, and the parent of the home directory is automounted.
  - Every python-lifecycle shard, `python.wrkslots.shard-6`, `rust.wrkslots.test` and `repository.check.wrkslots-debug-test` failed for one of three reasons:
    - "validation exclusion found no trusted root-owned, same-filesystem sticky parent". The Python fixtures and the Rust tests name `/tmp` itself as the trusted parent (`test_release_frozen_rust.py`, `test_lifecycle.py`, `validation_exclusion.rs`), so `tmp_path` must be on `/tmp`'s filesystem.
    - "cannot determine Git provenance ... cannot inspect ancestor marker <home parent>/.git: Operation not permitted". Inside the namespace shards' user and mount namespace, the automounted home parent refuses the lookup that the Git-marker walk up from a home-based `tmp_path` makes. Outside a namespace the same lookup gives ENOENT.
    - `test_strict_build_binds_under_box_tmp_and_confirms_masks`: "root view path is outside every trusted root".
  - `python.repository-infrastructure.validation-graph`: the #941 test reused the gate's inherited root. It now opts out (`AGENT_UTILS_PRIVATE_TMP=0`) and exercises the #941 path; the private-root path is covered in `test_private_tmp.py`.
- The other 5 were load or known reds on other trees: `python.wrkslots.shard-4` (#2065 census), `python.wrkslots.shard-14` (the #2372 per-test timeout in the mount-holder scan), `cross.herdr-run.differential` and `rust.merge-queue.test` (CPU timeouts, red on other trees 3 and 6 times since 11-25), `vibe-talk.check.discuss-browser` (#2175) and `vibe-talk.check.reload-place-browser` (red on other trees 3 times since 11-25).
- A sticky private root is not possible: a root-owned 01777 directory needs root. wrkslots' configured production exclusion root does qualify for `~/.aut` paths on that host (through the home directory's bind alias), but the tests name `/tmp`. Fixing that is #2415.

The scoped exception (interim until #2415, approved by the lead coordinator after the owner-facing liaison's review, which set four conditions):

1. Only `private_tmp.HOST_TEMP_STEPS` run under `with-private-tmp --host-temp`: the 9 python-lifecycle steps, the 16 `python.wrkslots` shards, `rust.wrkslots.test`, `repository.check.wrkslots-debug-test`, and #2065's future `python.wrkslots.host-*` steps.
   - Each gets a fresh root at `/tmp/aut-<uid>/<n>`. Every other step stays in `~/.aut`.
   - `test_only_the_listed_wrkslots_steps_run_in_the_host_temp_dir` fails the gate when any other step uses it, when a listed step does not, or when a step names `/tmp/aut` or a temp base.
2. Each root has an owner record (pid, process start time, PID namespace), written and flock-locked before the root exists and held for the run.
   - `sweep_host_temp` removes a root only when the lock is free and the recorded process is gone (pid absent or start time different). It never removes by name.
   - It logs each root's bytes and the total.
   - It runs at every host-temp activation and at gate start, and the daily tick runs `python3 scripts/private_tmp.py sweep`.
3. The wrapper measures the root every 10 s. Past 5,000,000,000 bytes it prints a named REFUSED line, stops the command (SIGTERM, then SIGKILL after 30 s), removes the root, and exits 73.
4. #2415 names this exception, and every DAG comment on a wrapped step points to it.
