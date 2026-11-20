---
title: 'wrkslots-seal-override-scan-residuals: cross-uid batches, unreadable stats, and the own-stamp override branch'
status: open
priority: 2
issue_type: bug
labels:
- wrkslots
- lifecycle
created_at: 2026-11-20T14:00:00+00:00
updated_at: 2026-11-20T14:00:00+00:00
---

# Description

[watermelon-full-preview] Residuals from the seventh cold review of #1869 (landed as c98f8428), all in the seal override's live-batch scan and its branches. (1) `_namespace_operator_live` skips an unreadable `/proc/<pid>/ns/pid` link, so a cross-uid batch (for example a root `unshare -p` judged by a non-root recover) is never matched and the override can retire its seal. (2) An unreadable stat on an NSpid-matching process does not count as live. (3) The own-stamp override branch in recover does not call `_assert_proc_consistent`. (4) Test gaps: the writer test calls only `_assert_proc_consistent`, and the F1 namespace test records a sleep child rather than namespace pid 1.

# Design

(1) On a non-ENOENT error reading a `/proc/<pid>/ns/pid` link, read the process status instead: treat it as live if `NSpid` has 2 or more fields, the innermost equals the recorded pid, and the start ticks match. (2) An unreadable stat on an NSpid-matching process counts as live. (3) Call `_assert_proc_consistent` in the own-stamp override branch, as the judge branches do. (4) Drive the real seal writer in the writer test, and record namespace pid 1 in the F1 test.

# Acceptance Criteria

- A cross-uid namespace batch whose ns link is unreadable is still found live by the override scan, in a test with real processes (or a faithful /proc double where real cross-uid is unavailable).
- An NSpid-matching process with an unreadable stat blocks the override.
- The own-stamp override branch refuses when /proc is inconsistent.
- The writer test drives the real writer and the F1 test records namespace pid 1.
- Focused seal/recover suites pass.

# Notes

Filed from the seventh review of #1869 wrkslots-adopt-reports-success-binds-nothing; the parent change is landed, so these stand alone. No new persisted field is expected.
