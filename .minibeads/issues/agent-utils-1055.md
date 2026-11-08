---
title: 'documents-browser-release-dep: the Documents browser check rebuilds the release binary inside its own memory cap'
status: closed
priority: 2
issue_type: bug
assignee: mu-tsconv
created_at: 2026-11-08T10:50:00.000000000+00:00
updated_at: 2026-11-08T10:54:00.000000000+00:00
closed_at: 2026-11-08T10:54:00.000000000+00:00
---

# Description

`vibe-talk.check.documents-browser` runs `cd vibe-talk && cargo build --release --locked && python3 tests/documents_browser.py` with `deps: [setup.contract-tools]` only. It is the one step in vibe-talk/tests.dag.yaml that builds the release binary itself instead of depending on `vibe-talk.build.release`. In a full gate the binary is usually already built, so the build is a no-op. In a solo `gate.py --rerun` on a freshly rebased tree it is not: the step compiles the release binary inside its own memory cap (rss baseline 1 GiB) and is OOM-killed. That happened twice on 2026-11-08 (21-27 s, "hit inner MemoryMax; 4 oom_kill event(s)"), including a rerun that also selected `vibe-talk.build.release`, because without a dependency the two ran side by side. It passed only after a separate rerun of `vibe-talk.build.release` first.

# Design

- Give the step `vibe-talk.build.release` as a dependency, like the other server-shaped browser checks, and drop the in-step `cargo build --release --locked`, so it runs against the binary the build step made.
- Keep its labels, resources and timeouts otherwise unchanged.

# Acceptance Criteria

- `common/bin/dagrun list --dag validation.dag.yaml` shows `vibe-talk.check.documents-browser` depending on `vibe-talk.build.release`.
- A solo rerun naming `vibe-talk.build.release,vibe-talk.check.documents-browser` on a tree whose release binary is stale builds once in the build step and passes the check, with no OOM kill.
- No step in vibe-talk/tests.dag.yaml other than `build.release` runs `cargo build --release` in its command (the graph contract test can assert this).

# Notes

Found by the devbig030 coordinator while landing the browser cap change (2532f6b2); filed at the lead's request.

## Resolution

[muse watermelon] `vibe-talk.check.documents-browser` now depends on `vibe-talk.build.release`
and no longer builds the release binary in-step. `py/tests/test_validation_graph_contract.py`
gains `test_only_the_release_build_step_builds_the_release_binary`: no vibe-talk step other than
`build.release` runs `cargo build --release`, and documents-browser depends on it (failed before
the fix, passes after). Proven by solo reruns on a stale tree (binary removed, then a source
touched): the build step rebuilds once under its own cap and the check passes.
