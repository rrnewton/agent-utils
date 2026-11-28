---
title: 'wrkslots-tests-temp-independent: wrkslots tests pick their trusted sticky root on the filesystem of tmp_path and survive unreadable ancestors, then drop the #2383 host-temp exception'
status: open
priority: 2
issue_type: task
labels:
- wrkslots
- validation
- tests
- tmp
created_at: 2026-11-27T17:46:00-05:00
updated_at: 2026-11-27T17:46:00-05:00
---

# Description

#2383 gives every gated step a private temp root under `~/.aut`. One group cannot use it, so it runs under a scoped, interim exception: `scripts/with-private-tmp --host-temp`, a fresh root at `/tmp/aut-<uid>/<n>`.

The group is `private_tmp.HOST_TEMP_STEPS`:

- the 9 python-lifecycle steps;
- the 16 `python.wrkslots` shards;
- `rust.wrkslots.test` and `repository.check.wrkslots-debug-test`;
- #2065's future `python.wrkslots.host-*` steps.

The exception carries four conditions, set by the owner-facing liaison through the lead coordinator:

1. Only those steps. A contract test fails the gate otherwise.
2. A sweep by owner record (pid and start time under an flock, never by name), at gate start and in the daily tick, that logs bytes.
3. A 5 GB bound per root, enforced with a named refusal.
4. This issue as the exception's named end.

Why the group needs `/tmp` (first gate of #2383, 2026-11-27, 14 red steps):

- The tests model wrkslots' ownerless validation exclusion, which requires a root-owned 01777 parent on the same filesystem as the fenced path. That requirement is a security property and stays as it is. The Python fixtures (`test_release_frozen_rust.py`, `test_lifecycle.py`) and the Rust tests (`validation_exclusion.rs`) name `/tmp` itself as that parent, so `tmp_path` must be on `/tmp`'s filesystem. On the gating host `~/.aut` is a different btrfs subvolume.
- The namespace shards walk Git markers up from `tmp_path` inside a user and mount namespace. There the automounted parent of the home directory refuses the lookup of its `.git` marker with EPERM, so wrkslots correctly refuses: it cannot prove provenance.
- `test_sandbox_root_spec::test_strict_build_binds_under_box_tmp_and_confirms_masks` expects its box tmp under a trusted root.

# Design

- A test helper picks the trusted root for `tmp_path`. It asks the production evaluator (`_trusted_validation_exclusion_root` and its Rust twin) for the first qualifying candidate among the production roots and `/tmp`, and skips with a stated reason when the host has none.
  - On the gating host, wrkslots' configured production validation-exclusion root already qualifies for paths under the home directory through its bind alias (checked 2026-11-27 with `_trusted_validation_exclusion_root`).
  - `WRKSLOTS_TEST_EXCLUSION_ROOTS` comes from the same helper.
- The namespace shards get a `tmp_path` whose ancestors can be read inside the namespace: the canonical non-autofs path of the same directory, or a namespace mount of the temp root. This must not change how wrkslots treats an unreadable ancestor.
- The sandbox root-spec test derives its trusted roots from `tmp_path`.
- Then remove `--host-temp` from those steps, `HOST_TEMP_STEPS`, and the contract test's allow-list.

# Acceptance Criteria

- Every step in `HOST_TEMP_STEPS` passes in the plain `~/.aut` root on the gating host, and the exception is removed.
- No wrkslots check is weakened: the same refusals fire for the same layouts.

# Notes

Filed 2026-11-27 at the lead coordinator's request, as the end of #2383's scoped exception (liaison condition 4).

## Proposal from #2383 (cl-tmpfix, 2026-11-27)

[opus 5.5] The way to end the exception, in four steps, after #2383's three gates:

1. A pytest helper and a Rust test helper find the trusted root for `tmp_path`.
   - They ask wrkslots' own evaluator (`_trusted_validation_exclusion_root` and its Rust twin) for the first qualifying candidate among the configured production roots and `/tmp`.
   - On the gating host, the configured production root already qualifies for paths under the home directory through its bind alias.
   - A host with no qualifying root skips the test with a stated reason.
   - The fixtures that name `/tmp` take the helper's answer, and so does `WRKSLOTS_TEST_EXCLUSION_ROOTS`.
2. The namespace shards get a `tmp_path` whose ancestors can be read inside the namespace: the canonical, non-automounted path of the same root. wrkslots' fail-closed provenance check stays as it is.
3. `test_sandbox_root_spec` derives its trusted roots from `tmp_path`.
4. `--host-temp` is dropped from the 27 steps. `private_tmp.HOST_TEMP_STEPS` is removed. The contract test `test_only_the_listed_wrkslots_steps_run_in_the_host_temp_dir` then requires that no step uses `--host-temp`.

Until this lands, liaison condition 2's daily sweep is `python3 <checkout>/scripts/private_tmp.py sweep`.
- It exits 0, prints one line per swept root, and ends with a total line.
- Run it from a checkout that contains #2383; an older checkout's `private_tmp.py` has no `sweep`.
- The lead coordinator adds it to the daily tick (team-tick).
