---
title: 'summary-link-test-host-tmp: the summary-link browser check keeps its server store in the host temp dir and leaked 747 MB there; audit every test and gate temp site that falls back to /tmp'
status: open
priority: 2
issue_type: bug
assignee: cl-tmpfix
labels:
- vibe-talk
- tests
- tmp
- cleanup
created_at: 2026-11-27T09:28:21-05:00
updated_at: 2026-11-27T09:28:21-05:00
---

# Description

`vibe-talk/tests/summary_link_browser.py` (the `summary-link-browser` validation step) puts the real server's config, store and message cache in `tempfile.TemporaryDirectory(prefix="summary-link-check-")`. With `TMPDIR` unset that is the host's `/tmp`. On 2026-11-24 a run left a 747 MB `summary-link-check-*` directory there.

- A step that dagrun stops at its timeout is killed with `cgroup.kill` (SIGKILL), so the `TemporaryDirectory` cleanup never runs. The check also never waits for the server it terminates before the directory is removed.
- Chromium's profile and Playwright's artifacts directory also go to the host temp dir, since the driver inherits the same `TMPDIR`.
- A failed Back check without `--screenshots` saves its screenshot to `tempfile.gettempdir()`, the host temp dir as well.

Agent work must not write to the host's `/tmp`. Many other tests and gate scripts fall back to `/tmp` the same way when `TMPDIR` is unset, and the host temp dir holds thousands of leaked test entries.

# Design

- The check keeps everything it writes under the vibe-talk build area (`vibe-talk/target/test-scratch/`): one private directory per run, named with the run's PID. The server's files, Chromium's profile and Playwright's artifacts (the browser's `TMPDIR`) and the failure screenshot all go there.
- The per-run directory is removed on success and on failure (exceptions and SIGTERM), after the server has exited. A SIGKILL can still leave one behind, but only inside the worktree's build area, so it goes away with the build output. The next run also sweeps old leftovers whose PID is dead.
- Audit the other temp-dir sites in tests and gate scripts (Python `tempfile`, Rust `tempfile` and `std::env::temp_dir()`), and check whether validate, gate or dagrun give steps a private `TMPDIR`. Fix only cheap, test-only cases here, and propose anything larger to the lead coordinator.

# Acceptance Criteria

- A run of the check, passed or failed, leaves no entry in the host temp dir (no `summary-link-check-*`, `playwright_chromiumdev_profile-*` or `playwright-artifacts-*` it made).
- After a run, passed or failed, its per-run directory under `vibe-talk/target/test-scratch/` is gone. Only a failure screenshot stays, and the failure message names it.
- The audit list (each site that can land in the host's `/tmp` with `TMPDIR` unset, plus whether validate, gate and dagrun set `TMPDIR`) is recorded in the Notes below.

# Notes

Filed 2026-11-27 9:28 AM ET at the lead coordinator's request.

## Audit: temp sites that land in the host temp dir with TMPDIR unset

[opus 5.5] cl-tmpfix, 2026-11-27, against main 925d170c. Python sites were classified from the AST: a `tempfile` call with no `dir` argument, or any `gettempdir()`. Rust sites are `std::env::temp_dir()` (the repository uses no `tempfile` crate). Shell sites are `mktemp` with no directory or `${TMPDIR:-/tmp}`. Literal `"/tmp/..."` strings in tests (156 Python, 185 Rust) were counted but not judged: most are fake paths in assertions.

The infrastructure gives no step a private TMPDIR:

- `scripts/validate.py`, `scripts/gate.py` and dagrun set no `TMPDIR` for steps. Every gated step inherits the caller's `TMPDIR`, which is usually unset, so it uses the host's `/tmp`.
- The only step with its own is `rust.agentctl.test`: a per-step `/dev/shm` directory, falling back to `$XDG_RUNTIME_DIR` (#869).
- validate gives pytest a private root through `PYTEST_DEBUG_TEMPROOT` (#941) and removes it at run end. That root, and every other validate scratch directory (global gitconfig, agentctl registry, NO_RESULT file, cross coverage, case timings), is itself made with `mkdtemp` in the host temp dir.
- dagrun ends a timed-out step with `cgroup.kill` (SIGKILL), so no in-process cleanup runs for it.
- So the gate does not shield tests: every site below can write to the host's `/tmp`, gated or not.
- Python's `tempfile` falls back to `/var/tmp` when `/tmp` is not writable. That is no escape: on at least one host, `/var/tmp` is the same volume as `/tmp`.

Gate scripts (33 sites):

- `scripts/validate.py`: 1135, 1150, 1165, 1298 (sweep), 1333, 2167, 2262, 2295, 2553, 2554.
- `scripts/gate.py`: 457, 2688.
- `scripts/run_pytest_shard.py`: 415, 702 (sweep), 703 (per-shard temp root), 799, 907, 948, 1012, 1044, 1089, 1148, 1174, 1195. The ones after 703 are self-test fixtures.
- `scripts/run_component_tests.py`: 451, 575, 652.
- `scripts/case_timings.py`: 1013. `scripts/check_issue_parse.py`: 135, 169. `scripts/check_standalone.py`: 459.
- Shell: `common/bin/rs-source-fingerprint`: 24; `validation/rust-only/smoke.sh`: 35.

Other scripts (28 sites):

- `scripts/check_*.py` self-tests (13 files, 19 sites), and `scripts/sweep_test_mounts.py`: 90, 456.
- `scripts/rebase-delta-guard`: 209, 325, 573.
- `vibe-talk/scripts/run.sh`: 569. `vibe-talk/scripts/test-run-sh.sh`: 44.
- `common/bin/wrkslots-soak`: 276. `py/wrkslots/spec/run_tlc.sh`: 41.

Tests (810 sites in 438 files), as sites/files:

- Python:
  - vibe-talk 209/167. Of these, 165 are browser walks, and 31 of those start the real server inside a `TemporaryDirectory`, as this check did.
  - `py/tests` 57/7, 42 of them in `test_cli.py`.
  - `cross/` differential harnesses 49/4.
  - `py/wrkslots/tests` 6/4.
- Rust:
  - agentctl 182/104 (sent to `/dev/shm` inside the gate by its step).
  - dagrun 109/41, wrkslotsd 45/17, herdr-run 27/12, vibe-talk 20/11, merge-queue 19/11, agent-teams 18/11, gh-paced 17/11.
  - chat-subscription (slack, plugin, fake) 18/8, agent-queue 6/4, agent-usage 6/6, local-ci-hub 6/6, tick-hub 5/4.
  - testrun-core, pr-landing-planner, agent-harness, change-classifier, claude-stream-session and wrkviz: 8/7 together.
- Shell: three `py/wrkslots/tests` acceptance and e2e scripts.

Two sites are not in these counts:

- Every Playwright walk's Chromium profile (`playwright_chromiumdev_profile-*`) and artifacts directory (`playwright-artifacts-*`) go to the driver's `TMPDIR`, with or without a `tempfile` call in the walk.
- pytest's `tmp_path` uses validate's root inside a gate, and `/tmp/pytest-of-USER` outside one.

Regenerate the raw lists with `git grep -n -E 'tempfile\.(mkdtemp|TemporaryDirectory|NamedTemporaryFile|mkstemp|gettempdir)\b' -- '*.py'` and `git grep -n -E 'env::temp_dir\(\)' -- '*.rs'`. Leave out calls that pass `dir=`.

What already sits there: on 2026-11-27 one host's `/tmp` held, among others, 3000 `vibe-talk-readcli-*`, 1971 `agent-teams-relay*`, 498 `agent-teams-topology-*`, 440 `vibe-talk-startup-*-rate-limited`, 406 `agent-utils-gitconfig-global*`, 243 `wrkslots-opath-target*`, 212 `merge-queue-approve-test-*`, 134 `agentctl-inbox-test-*`, 133 `vibe-talk-chrome-unpack*` and 80 `playwright*` entries, plus three `cross-view-back-*` server directories from earlier days. #2166 tmp-stale-sweep covers six of these patterns after the fact.

Tests that leave entries even after a passing run. These were reported by a sibling lane after `cargo test -p agentctl --lib -- chat_` and confirmed on main. All three use `std::env::temp_dir()`:

- `rs/agentctl/src/chat_history.rs:887` `tempfile_dir()`: it removes `agentctl-chat-history-test-<pid>` before use but never after.
- `rs/agentctl/src/chat_event_sink.rs:1105`: the inline `temp_dir("carry-fresh3")` is missing from the cleanup loop.
- `rs/agentctl/src/chat_runtime.rs:39655`: the first `temporary("receipt-check-mark-ack")` root is shadowed by the `-old` root at 39666 and never removed.

## Fixed here (test-only)

- New `vibe-talk/tests/browser_scratch.py`, whose `run_scratch(prefix)` gives a run its own directory under `vibe-talk/target/test-scratch/`, named with the run's PID. It points `TMPDIR` at that directory's `tmp/` for the run, so the server, Playwright's driver and Chromium put their files there. It removes the directory on success, exceptions and SIGTERM, restores `TMPDIR`, and first sweeps leftovers whose PID is gone and that have stood untouched for an hour. A SIGKILL leftover stays inside the worktree's build area.
- `summary_link_browser.py` uses it, waits for the server to exit before removal, and saves a failed Back check's screenshot without `--screenshots` to `target/test-scratch/` instead of the host temp dir. Before, the server was terminated but never waited for, so it could still be writing while its directory was removed.
- `cross_view_back_browser.py` (same pattern, and it imports from `summary_link_browser`) uses it too.

## Proposed follow-up (not done here; for the lead coordinator)

- One root for the whole run. validate (and gate.py's own scratch) would export `TMPDIR`, `TMP` and `TEMP` for the whole graph: a private per-run root outside the host's `/tmp`, removed at run end (passed, failed or SIGTERM), with a stale sweep for SIGKILL leftovers. These are the #941 mechanics; the pytest root and the other validate scratch would nest inside it. That covers every gated site above at once, with no per-test edits.
  - The root must be short. A Unix socket path is limited to 108 bytes, and tests bind sockets under `TMPDIR` (#447, #869), so a root inside a worktree is too long.
  - Candidates are a short per-user directory on disk, such as `~/.cache/agent-utils/tmp/<run>`, or the per-user runtime dir. The runtime dir is tmpfs and size-capped.
  - Tests that pin their own location keep it: `rust.agentctl.test`'s `/dev/shm` (#869) and the socket probe's runtime-dir choice (#1230).
- The same for one command outside the gate: a small `scripts/with-private-tmp CMD` wrapper, used for focused runs.
- The 31 browser walks that start the real server can move to `browser_scratch.run_scratch`. That is mechanical, but each walk needs a focused run.
- The three agentctl test leaks above: a small agentctl-only change, kept out of this vibe-talk change so its gate stays vibe-talk only.
- Production sites (18 Rust, about 80 Python, for example the agent-teams relay and the wrkviz export, #2140) are out of scope here.
