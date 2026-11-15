---
title: 'wrkslots-abort-create-failed-hook: abort-create forgives a failed submodule-init hook'
status: closed
priority: 1
issue_type: bug
assignee: mu-abortfail
labels:
- wrkslots
created_at: 2026-11-15T07:50:50+00:00
updated_at: 2026-11-15T07:50:50+00:00
---

# Description

Direct follow-up to #1337 create-abort-hook-partial-submodule (landed 6cca45be). Reported read-only by another team on this host: `wrkslots recover --abort-create` on an interrupted CREATE whose post-provision hook 0 (`git submodule update --init`) FAILED (rc 128) refuses:

    cannot abort changed checkout ... source state is dirty (... <submodule>); preserve it for inspection

The failed init left the submodule partially initialised. #1337 forgives that partial-init dirt only when the journal holds the hook at `running`, not at `failed`, so a CREATE with a failed hook could not be retired by any documented path. Code: `py/wrkslots/cli.py`, `_recover_create` and `_abort_create`.

# Design

- Extend the #1337 forgiveness to hook status `failed` and `failed-to-start`, but ONLY for the hook that runs the submodule update/init (`git submodule update --init`, any spelling with those tokens). A failed hook of any other kind can leave authored or arbitrary output, so it keeps refusing.
- Keep every #1337 condition: the gitlink still names the recorded commit; the submodule was uninitialized at create start; the dirt is deletions-only or never-initialized; no commits outside remote-tracking refs; no stash or operation in progress; no other dirt. Reuse the #1337 partial-init shape check (`_submodule_partial_init` / `_hook_partial_init_dirt`) unchanged.
- Reproduce the real failure shape as a fixture: make the init actually fail with a nonexistent submodule URL (local only, no network). Git clones every top-level submodule before checking any out, so a failed clone leaves the good submodule cloned but never checked out and the broken one never cloned. Do not reuse the killed-mid-run fixture.
- Negative fixtures must still refuse and preserve everything: a failed non-init hook; a moved gitlink; authored files; edited submodule content.
- Python edition only. Per the #1337 parity note, the Rust recover writer implements only `--abort-import`.

# Acceptance Criteria

- A fixture whose submodule-init hook really fails (nonexistent URL) aborts cleanly with `recover --abort-create`; before the fix it reproduces the `source state is dirty` refusal.
- The same forgiveness holds when the journal holds the init hook at `failed-to-start`.
- Fixtures with a failed non-init hook, a moved gitlink, authored file changes, or edited submodule content still refuse and preserve everything.
- The parity gap is recorded in the Resolution.
- The gate passes at normal priority and the fix lands on `main`.

# Notes

Assigned by the coordinator from the devbig030 ledger block. Normal priority; the reporting team retires its slot with its own pin bump after this lands.

## Resolution

Landed on main (SHA in the delivering commit). `_abort_create` now forgives, in the checkout whose hook the journal holds at `failed` or `failed-to-start`, the same partial-init dirt #1337 forgives at `running`, provided the recorded hook command runs `git submodule update --init` (new `_hook_runs_submodule_init` token check in `py/wrkslots/cli.py`; `_hook_partial_init_dirt` and every other #1337 condition unchanged). Fixtures extend the #1337 abort suite in `py/wrkslots/tests/test_lifecycle.py`: a real rc-failure create (nonexistent submodule URL; good submodule left cloned but never checked out, broken one never cloned) aborts cleanly, as does its `failed-to-start` journal variant and the cloned-nothing variant; a failed non-init hook, a moved gitlink, authored dirt, and edited submodule content still refuse and preserve everything. Parity note: create-journal `--abort-create` stays with the Python edition - the Rust recover writer implements `--abort-import` only (`rs/wrkslotsd/src/recover_main_write.rs`), so there is no second dirty check to keep in agreement until that recovery moves.
