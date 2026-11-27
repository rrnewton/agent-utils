---
title: 'wrkslots-deep-path-tests-own-device: tests that hold paths deeper than PATH_MAX build them on a private tmpfs so concurrent slot-use censuses stay decidable'
status: open
priority: 1
issue_type: bug
assignee: cl-flaketriage
labels:
- wrkslots
- tests
- census
created_at: 2026-11-26T22:00:00-05:00
updated_at: 2026-11-26T22:00:00-05:00
---

# Description

The slot-use census of both wrkslots editions reads every process's `/proc` links host-wide. A link to a path deeper than `PATH_MAX` cannot be read (`ENAMETOOLONG`); when it lands on the same filesystem device as the slot being checked, the census must place it by walking its parents (`#618`, and `#2341` for the Rust edition), and a deep FILE cannot be placed at all, so that census refuses as indeterminate. Pytest's temporary directories, where every test slot lives, all share one device. So while any test holds a deep path under its temporary directory, every concurrent census of a test slot on the host (other shards of the same gate, other gates, other lanes' runs) is judged against it: the pre-`#2341` Rust census exits 1 on a deep directory, and both editions refuse on an unplaceable deep file.

The standing risk is the long-path census tests (`py/wrkslots/tests/test_census_long_paths.py` holds deep directories under its temporary directory while it runs; `#2341` adds a differential that holds a deep file). Tonight's trigger was a manual probe of `#2341` that held a deep file under the temporary directory for about a minute: six wrkslots tests in another lane's gate (shards 0, 5, 8, 13) refused with "cannot place that directory by its parents: Not a directory" on that probe's `/proc/PID/fd/3`.

# Design

Every test that holds a path deeper than `PATH_MAX` builds it, and the slot or target it is placed against, under a private directory on a filesystem no other test's slot uses: a fresh directory under `/dev/shm` (a tmpfs with its own device), removed at teardown. Every concurrent census of a slot elsewhere then proves those links foreign by device (both editions check the device first) and stays decidable, while the test still exercises the same-device path because its own slot is on that tmpfs too. When `/dev/shm` is missing or shares the temporary directory's device, the tests skip with a named reason rather than poisoning the host. No root, no namespaces, no per-gate resource (a gate-local resource cannot protect other gates).

# Acceptance Criteria

- `test_census_long_paths.py` (and `#2341`'s differential when it lands) creates every deep tree and its target under a private `/dev/shm` directory, removed at teardown, and skips with a named reason where that is not available.
- A test proves isolation: while a helper holds an unplaceable deep file on the private tmpfs, a Python and a Rust census of a slot under the temporary directory both report the slot unused.
- No test leaves a deep tree or a helper process behind; no root and no wall-clock budget (owner rule 346).

# Notes

Found by `cl-flaketriage` (coordinator's assignment, 2026-11-26) from gate `16c5d41f`'s wrkslots shards; the probe PIDs in its logs were this lane's own `#2341` helpers. Lands before `#2341`, which rebases on it. Related: `#618`, `#651`, `#2341`.
