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
