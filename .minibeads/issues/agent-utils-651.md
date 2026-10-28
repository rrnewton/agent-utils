---
title: 'wrkslots-census-test-hermetic: census-sensitive tests need their own fixture filesystem'
status: closed
priority: 2
issue_type: task
created_at: 2026-10-28T14:24:08.912375344+00:00
updated_at: 2026-10-28T22:54:47.944091+00:00
closed_at: 2026-10-28T22:54:47.944091+00:00
---

# Description

Since #618, the wrkslots process-use census proves an unreadable descriptor foreign only by filesystem device. Test fixtures created under pytest's tmp_path share the /tmp device with every other process on the host, so an unrelated host process holding an unreadable descriptor whose target is also on the /tmp device (measured: a foreign test binary's fd 26, ENAMETOOLONG) makes census-sensitive tests refuse as indeterminate. This now fails gate steps on every run on that host (the Rust remove differential tests in python.wrkslots.shard-0/1; the same mechanism also hits packages.standalone.wrkslots and two Python lifecycle host tests).

Fix: census-sensitive tests must place their fixture slot on their own filesystem, e.g. a private tmpfs such as /dev/shm (a distinct device from /tmp), or skip with a clear reason when no distinct-device filesystem is available. Production slots are separate filesystem images, so the device proof is correct there; this issue is only about test hermeticity.

Scope order (mu-rust lane): (1) py/wrkslots/tests/test_remove_write_rust.py fixtures on a distinct device with a clear skip; (2) survey the other census-sensitive harnesses (check_standalone lifecycle, Python lifecycle host tests) and apply the same treatment or record why they differ. Filed at coordinator direction after #562 slice 38.

## Progress (2026-10-28, mu-rust)

Part (1) landed on main as `32e27aee`: the census-sensitive remove differential tests (`test_remove_matches_python`, `test_remove_event_log_replays_through_rust_reader`, `test_remove_ignored_file_still_removes`) now build fixtures on /dev/shm (distinct device from the pytest tmp dir) and skip with a clear reason when no distinct device exists. All 11 remove tests pass on the loaded host; gate 57 shows `python.wrkslots.shard-1` passing in-gate (549 passed) and shard 0 passing on solo rerun (550 passed).

Still open (part 2): `packages.standalone.wrkslots` and the Python lifecycle host tests still create their fixture roots under /tmp and can still be refused by a same-device foreign unreadable descriptor. They pass in a PID namespace. Applying the same distinct-device treatment to those harnesses is the remaining work in this issue.


## Closed (2026-10-28, mu-rust)

Part 2 landed on main as `1b3e01d2`, at coordinator direction: instead of namespace machinery, #618's device proof now also applies to the find census's stderr. `_same_uid_indeterminate_processes` and `_allow_only_vanished_process_diagnostics` drop a `File name too long` diagnostic naming `/proc/<pid>/fd/<n>` when stat of that descriptor proves a filesystem device different from every census target's; callers pass their targets' devices. A same-device descriptor or a failed stat keeps refusing (fail-closed, per the #618 correction).

Evidence: gate 61 (`/tmp/rust-campaign-gate61.log`) — `packages.standalone.wrkslots` and all three lifecycle host shards pass in-gate on the host that previously refused them; the remove differential passes on the host (11/11) without a PID namespace. New unit tests in `test_census_foreign_fds.py` cover the foreign-device drop through both classifiers, the same-device refusal, and the failed-stat refusal. The only remaining gate-61 failures were shard/infra timeouts under load, each passing alone (infra 113; shards 563 + 563).

Residual, by design: a foreign descriptor whose target is on the very same device as a /tmp fixture and unreadable stays indeterminate. On production slots (separate filesystem images) the proof applies; the two stale foreign processes that triggered this issue are the owner's to kill, and the fix does not depend on that.
