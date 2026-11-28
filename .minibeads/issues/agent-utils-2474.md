---
title: 'image-status-test-bare-107: the wrkslotsd image-status test rules out Errno 107 by the bare digits 107, which the temp path in the same message can contain'
status: open
priority: 1
issue_type: bug
assignee: cl-flaketriage
labels:
- wrkslots
- tests
- rust
created_at: 2026-11-28T11:10:00-05:00
updated_at: 2026-11-28T11:10:00-05:00
---

# Description

`rs/wrkslotsd/src/image_status.rs` (`tests::usage_and_rendering_on_temp_images`) checks that a missing image file's stat error wins over the same entry's statvfs error. Line 283 on 2026-11-28 main:

    assert!(!first_error.contains("107"), "{first_error}");

It means "the error is not Errno 107 (Transport endpoint is not connected)". But the error text embeds the fixture path, `<temp dir>/wrkslotsd-image-status-<pid>-<n>/.../slot.img`.
- A gate step's temp dir is its private root, named by the wrapper's host pid (`/tmp/aut-<uid>/<pid>`).
- The test's own pid is in the path too.
- Whenever either contains the digits 107, the assertion fails although the error is the expected `[Errno 2] No such file or directory`.

Seen on 2026-11-28 in gate 1795880439235867443-78e85b6a: both `rust.wrkslots.test` and `repository.check.wrkslots-debug-test` failed this one case. Their roots were `/tmp/aut-<uid>/1075891` and `/tmp/aut-<uid>/1075922`. The case passed in the 18 other kept step logs.

A sweep for the same pattern:
- Negative substring checks on bare digits in Rust tests: two others, `rs/agentctl/src/chat_runtime/reply_reminders.rs` (`!head.contains("033")`) and `rs/dagrun/src/scheduler.rs` (`!notice.contains("1048576")`). Both check fixed text with no path in it, so they are safe.
- Python tests have no such negative check. Positive bare-digit checks (for example `assert "17" in output`) cannot fail by chance; they can only pass by chance.

# Design

- Assert on the errno text itself: the error contains neither `[Errno 107]` nor `Transport endpoint`. Not the bare digits.
- Control: the test with `TMPDIR` set to a directory whose name contains 107 fails on today's main and passes with the fix.
- Test-only; no product change; no wall-clock budget (owner rule 346).

# Acceptance Criteria

- `usage_and_rendering_on_temp_images` passes with a temp root whose name contains 107, in both the release and the debug profile.
- `rust.wrkslots.test` and `repository.check.wrkslots-debug-test` pass in a gate.

# Notes

Filed by `cl-flaketriage` at the coordinator's request (2026-11-28; number pre-assigned), from the #2464 gate.

Related:
- #2383: the gate's private temp root, whose pid-named directory carries the digits.
- #2464: the change whose gate hit it (unrelated to the case).
