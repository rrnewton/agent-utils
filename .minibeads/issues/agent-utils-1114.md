---
title: 'fake-exec-etxtbsy: merge-queue tests that write a fake executable and immediately exec it race fork'
status: open
priority: 2
issue_type: bug
assignee: mu-predict
created_at: 2026-11-09T04:14:26.000000000+00:00
updated_at: 2026-11-09T04:14:26.000000000+00:00
---

# Description

`validate::unit::a_dagrun_journal_deterministic_failure_stops_the_run_early` failed in the #1100 full gate with `cannot start .../fake-dagrun: Text file busy (os error 26)`, turning the expected deterministic-failure run into a refusal; the same test passes alone. This is the classic fork/exec race, not host load: the test writes the fake executable while another thread in the same test process forks, the child inherits the still-open writer descriptor, and the subsequent exec of the just-written file gets ETXTBSY. The fake-dagrun helpers in `validate.rs` (around lines 2309 and 2801) write, chmod and immediately execute the same path; other merge-queue test helpers across `validate.rs`, `donor.rs`, `serve.rs` and the integration tests use the same write/chmod/exec shape and may share the race.

# Design

- Make write-then-exec atomic from the executor's point of view: write the fake to a temporary sibling name, flush and close it, chmod it there, then rename it into place (or open with O_CLOEXEC and close before exec), through one shared helper where a shared helper already exists.
- Inventory the merge-queue test helpers that write a script/binary and then execute it (the `PermissionsExt` 0o755 sites in validate, donor, serve and integration tests at filing time); convert every site the race can reach, or record why a site cannot race (single-threaded process, no concurrent fork).
- Do not retry loops around the exec as the fix: the race window is the open descriptor, not the exec call.

# Acceptance Criteria

- The deterministic fake-dagrun tests pass repeatedly under parallel `cargo test -p merge-queue` load (many threads forking), with no ETXTBSY refusal.
- The inventory in the closing note lists each write-then-exec helper found and whether it was converted or is race-free by construction.
- No production code changes behaviour; test-only change unless a production helper proves to share the shape.

# Notes

Found by the #1100 full gate on 2026-11-08 (590-case merge-queue step, one failure; classified by the lead coordinator). Filed after #1100 landed, as directed.
