---
title: 'dagrun-libtest-attribution: libtest lines whose test name has a space are dropped, and a start line with output glued on is read as an end with a garbage verdict'
status: open
priority: 2
issue_type: bug
assignee: cl-redlogs
labels:
- dagrun
- attribution
- flake-triage
created_at: 2026-11-28T13:07:00-05:00
updated_at: 2026-11-28T13:07:00-05:00
---

# Description

dagrun's test-boundary recognizer reads Rust libtest output as `test NAME ... VERDICT` (an end) or `test NAME ... ` (a start). The test boundaries in the step journal, and the culprit named when a step hangs, come from it. Both engines have the same two defects: `rs/dagrun/src/attribution.rs` `recognize` and `py/dagrun/attribution.py` `recognize`.

- **Names with a space are dropped.** The recognizer refuses any libtest name that contains a space. libtest itself prints spaces in two shapes, so those tests get no boundary at all:
  - a mode annotation after the name: `test some::name - should panic ... ok` (also ` - compile fail` and ` - compile` on doc tests);
  - doc-test names: `test src/lib.rs - module::item (line 12) ... ok`.
- **A split line becomes an end with a garbage verdict.** A serial libtest run without output capture prints `test NAME ... ` when the test starts and the verdict only when it ends, so the test's own output (tracing lines, for example) is glued onto the start line: `test NAME ... 2026-11-28T10:00:00Z INFO starting`. The recognizer reads that as the END of NAME with the verdict `2026-11-28T10:00:00Z`.
  - The journal gets a `test_end` with a meaningless verdict.
  - Worse, NAME counts as completed the moment its first output line ends. If NAME then hangs, the culprit is not named.

Reported by a consuming project's infrastructure lane from its own step logs. Those logs stay on the reporter's hosts; the fix is built from the code and synthetic fixtures.

# Design

Same change in both engines, kept in parity (the cross differential compares their journals byte for byte):

- **Names.** Strip one trailing libtest mode annotation (` - should panic`, ` - compile fail`, ` - compile`) from the name. Then accept a name with spaces only in the doc-test shape `PATH - ITEM (line N)`. Any other name with a space stays unrecognized, so prose such as `test that it works ... done` is still not a boundary.
- **Verdicts.** `test NAME ... REST` is an end only when REST starts with a libtest result word: `ok`, `FAILED`, `ignored` (also `ignored, <reason>`, recorded as `ignored`) or `bench:` (recorded as `bench`). Any other REST is a split line: a START of NAME whose output followed on the same line.
- **Closing a split-line test.** The verdict of a split-line test is printed after its output, usually as a bare line, which stays unrecognized (a bare `ok` is too common to trust). libtest prints start lines only when it runs tests one at a time. So when a libtest line names another test while a test opened by a libtest start line is still in flight, the open test is closed: it counts as completed and the journal gets its `test_end` with the verdict `unknown`. Without this rule every split-line test would stay in flight for good, and a later hang would be blamed on it.
- `##TEST-START`/`##TEST-END`, pytest and TAP recognition are unchanged.

# Acceptance Criteria

- Synthetic fixtures in both engines' tests:
  - `test a::b - should panic ... ok` ends `a::b` with `ok`;
  - `test src/lib.rs - m::f (line 3) ... ok` ends `src/lib.rs - m::f (line 3)`;
  - a split line starts its test, never ends it;
  - the next serial test closes it with `unknown`;
  - a test that hangs after a split line is named the culprit;
  - prose with spaces and a `...` stays unrecognized.
- The same inputs give the same events and journal records in both engines.
- No wall-clock budget in any new test (owner rule 346).

# Notes

Filed 2026-11-28 at the lead coordinator's request; number pre-assigned.
Related: #2409 (red-step evidence keeps these journals), #2065 (census test flakes found through them).
