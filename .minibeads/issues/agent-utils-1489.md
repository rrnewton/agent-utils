---
title: 'gate-defer-patterns-and-observe: list and exclude unit patterns, plus an observe-only defer mode'
status: closed
priority: 2
issue_type: feature
created_at: 2026-11-13T14:02:55.214906287+00:00
updated_at: 2026-11-13T14:57:11.958138913+00:00
closed_at: 2026-11-13T14:57:11.958138812+00:00
---

# Description

[watermelon-full-preview] Follow-up to #1406 gate-defer-merge-queue. A consuming host cannot enable gate-defer yet: one broad unit glob would also catch always-active or failed units that must never defer a gate, and the host wants to observe what would defer before enforcing it.

# Design

1. defer_unit_pattern accepts a LIST of globs as well as a single string (backward compatible); new defer_unit_exclude = [globs] excludes units a broad glob catches (for example an always-active worker unit). Matching is against active user units only, as now; excluded names never defer. 2. Observe-only mode: defer_mode = "observe" (default "enforce"). In observe mode gate.py never waits, but appends one JSON line per would-have-deferred check (time UTC, gate id, which signal: unit names / hold labels / quiet-command rc) to a host log, default ~/.local/state/agent-utils/gate-defer-observe.jsonl (configurable defer_observe_log). gate.py status shows the mode. 3. Config validation for the new keys: malformed values refuse the start with exit 2, as today. Docs in the gate.py docstring and --help (the gate user guide), and tests for list + exclude matching, observe logging without waiting, and unchanged enforce behaviour. No wall-clock waits in tests.

# Acceptance Criteria

- defer_unit_pattern as a list defers on any listed glob; a single string behaves exactly as before. - A unit matching defer_unit_exclude never defers, even when it also matches an include glob. - In defer_mode = observe, a gate whose unit is active runs immediately and one JSON line naming the unit is appended to the observe log; gate.py status shows the observe mode. - Malformed new keys (bad mode, non-string list element, non-list exclude, non-string log path) refuse the start with exit 2. - Existing defer tests and strict mypy on changed files pass; lands through the normal gate.

# Notes

Follow-up to #1406 gate-defer-merge-queue (scripts/gate.py, landed cb80bf40). Needed by a consuming host before it can enable gate-defer. Number assigned by the coordinating lane for this work.

## Resolution

[watermelon-full-preview] Landed in scripts/gate.py. defer_unit_pattern now accepts a single glob or a list of globs (backward compatible), probed once per pattern against active user units only; defer_unit_exclude = [globs] removes matching names before anything defers, so excluded units never defer. defer_mode = "observe" (default "enforce") never waits: at the pre-preflight and pre-grant checks it appends one JSON line (UTC time, gate id, unit names / hold labels / quiet-command result) to the observe log (default ~/.local/state/agent-utils/gate-defer-observe.jsonl, configurable defer_observe_log / $GATE_DEFER_OBSERVE_LOG; $GATE_DEFER_MODE overrides the mode) and proceeds; gate.py status and queue show the mode and, in observe mode, the log path. New keys validate like the existing ones: malformed values refuse the start with exit 2. Docs in the module docstring and --help. Tests in py/tests/test_gate_admission.py cover list + exclude matching, config validation, observe logging without waiting, and enforce with a list pattern; existing defer tests unchanged. Validated with a fresh cpuset local validate and a GATE_SLOTS=3 gate, both green; strict mypy shows only the 2 pre-existing errors also present on the untouched base.
