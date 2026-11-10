---
title: "census-single-pass: the process census walks /proc once per slot instead of once per command"
status: open
priority: 2
issue_type: bug
assignee: cl-census
created_at: 2026-11-10T00:40:00.000000000+00:00
updated_at: 2026-11-10T00:40:00.000000000+00:00
---

# Description

The wrkslots process/path census walks /proc once for each slot it checks. A consumer measured ~12 s per slot,
~22 minutes for 111 slots at 4,255 processes (2026-11-09). A single pass over /proc that maps each process's
references (cwd, root, exe, fds, maps, and the same-UID and privileged census results) to the slots they fall in
takes about 1 s.

# Design

Build one census per command: walk /proc once, collect each process's references with the same readers and the
same error classification as #1173 (ESRCH/ENOENT = gone; EACCES and others = indeterminate), and index them by path
prefix and (dev, ino) so each slot's question is a lookup. Keep any per-slot follow-up (lsof fallback, fenced
scans after a fence) as it is, since those are deliberately after-the-fact. #1174 (the Rust census reads root and
maps) lands first, so both editions read the same entries.

# Acceptance Criteria

- One /proc pass per command for remove, remove-agent-batch, audit and doctor/status, in both editions.
- Results identical to today's per-slot census: a differential test over a fixture with slots used by cwd, root,
  exe, fd and mmap holders, plus a vanishing process.
- A measurement at 100+ slots and a few thousand processes (synthetic), before and after, recorded in Notes.
- Vanished processes handled as in #1173.

# Notes

Requested by the lead coordinator (2026-11-09) from a consumer's measurement. For cl-census, after #1173 and #1174.
Related: #1172 audit-lock-hold (its row checks are a separate cost).
