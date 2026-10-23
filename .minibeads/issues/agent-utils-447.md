---
title: 'validation-host-fixtures: provision timezone and bounded temporary test storage'
status: open
priority: 2
issue_type: task
created_at: 2026-10-23T11:45:46.037675718+00:00
updated_at: 2026-10-23T11:45:46.037675718+00:00
---

# Description

Local selected validation needs portable fixture provisioning. On this host tzdata is installed but tzdata-legacy is absent, so TZ=right/UTC becomes the literal abbreviation right and the leap-second calendar test fails. Official matching legacy data staged in user space restores all 26 prompt-time tests. The heavy agentctl Rust suite timed out on journal waits with default disk fixtures; executable RAM fixtures complete 1350 cases in about 164 seconds. Global RAM TMPDIR is unsuitable: long paths overflow Unix sockets and retained temporary fixtures make the small cross-environment Python step hit its 320 MiB cap. Passing full selection keeps Python fixtures on disk and prefixes only the Rust test command with a short RAM TMPDIR; all 32 steps pass without changing assertions, timeouts, labels, dependencies or resource hints.

Follow-up: make setup/preflight provide the real leap-second fixture dependency and support an explicit host-owned per-Rust fixture placement setting, deriving from the canonical graph without a maintained command-list fork. Keep ordinary invocation portable, preserve socket-length and memory tests, and fail loudly when host requirements are absent. Current feature agent-utils-446 is delivered; this is the remaining host/bootstrap task.
