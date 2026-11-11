---
title: "failure-lines: name the failing test or suite in the donor receipt and ejection report"
status: closed
priority: 2
issue_type: feature
assignee: ac-mqclose
created_at: 2026-11-10T09:26:00.000000000+00:00
updated_at: 2026-11-10T09:26:00.000000000+00:00
---

# Description

A change was ejected on "1 of 29 script test suites failed" with no suite name. The
donor receipt's bounded excerpt keeps each failing node's tail, and the line naming
the failed suite had scrolled out of it. Queue-host agents cannot reach donor hosts,
so nobody could diagnose the ejection from the queue host.

Build:

1. Failure lines in the receipt: for each failing node, also carry the lines that
   name the failure, bounded (at most 40 lines and 8 KB per node), picked by
   patterns — `FAILED`, `panicked at`, `test result: FAILED`, `error[` — plus a
   configurable extra pattern list in the queue config. The first such line goes
   into the verdict detail and into the ejection report
   (`runs/<run>/ejections/<pr>.md`), so the eject event names the failing test
   or suite.
2. Optional, if small: a donor uploads the full log of a failing node, bounded in
   size, next to the receipt, so the queue host can read it without host access.

# Acceptance Criteria

- A receipt whose failure line sits far before the node's tail still names the
  failing test or suite in the verdict detail and the ejection report.
- The per-node bounds (line count and bytes) hold.
- Extra patterns from the queue config are honoured.
- Tests cover all three.
