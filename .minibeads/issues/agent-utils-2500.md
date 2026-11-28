---
title: 'wrkviz-serve-log-order: the wrkviz serve differential compares the request log line by line, and the two editions can order the lines around a 404 differently'
status: open
priority: 2
issue_type: bug
labels:
- wrkviz
- tests
- editions
created_at: 2026-11-28T15:30:00-05:00
updated_at: 2026-11-28T15:30:00-05:00
---

# Description

`py/tests/test_wrkviz_serve_rust.py::test_serve_matches_python` failed in gate 1795894776097312074-bd122e61 (`python.wrkviz.shard-2`, 2026-11-28) on `assert rs_log == py_log`. The banners and observed responses matched. Only the order of two request-log lines around one 404 differed:
- Python wrote the request line (`"GET /missing.txt HTTP/1.1" 404 -`) and then the error line (`code 404, message File not found`);
- Rust wrote them the other way round.

Either one edition writes these two lines from different threads or streams, so their order is not fixed, or the editions differ in a fixed order. Which one is not established.

# Design

- Establish whether either edition's order varies from run to run (repeat the case, compare each edition with itself).
- If an edition's order is not fixed, make the comparison order-insensitive within one request: compare per-request sets of lines.
- If the editions differ in a fixed order, make them match, whichever order the Python edition's server library fixes.
- No wall-clock budget.

# Acceptance Criteria

- The cause is named.
- The differential passes repeatedly in its shard in a gate.

# Notes

Filed by `cl-flaketriage` at the coordinator's request (2026-11-28; number pre-assigned), from the #2493 gate. Unassigned.
