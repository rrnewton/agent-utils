---
title: 'validation-overhaul: finish full validation in under ten minutes'
status: open
priority: 0
issue_type: epic
labels:
- validation
- performance
created_at: 2026-09-25T04:10:29.740062972+00:00
updated_at: 2026-09-25T11:39:54.962641219+00:00
---

# Description

[gpt-5.6-sol] Audit and rebuild validation so affected-change runs are sharply selective while the complete portable, current-toolchain regression contract executes as a real dagrun graph with measured, justified parallelism.

# Design

One source of truth will classify paths, compute dependency closure, and assemble tool-owned DAG fragments. dagrun gains strict load-time includes with namespaces; execution profiles and per-test timings drive resource choices.

# Acceptance Criteria

A clean full validation run completes in under 10 minutes on the target host; selective documentation and single-tool edits complete substantially faster; every executable gate and every `py/` pytest case is inventoried and ranked by measured time, while other runner cases are attributed to a measured owning gate; expensive coverage receives a retain, optimize, relocate, or remove rationale; local and CI planning share one source; unknown paths fail safe; all changes are validated and pushed.
