---
title: 'dagrun-includes: compose validation DAG fragments at load time'
status: open
priority: 0
issue_type: task
labels:
- dagrun
- validation
depends_on:
  agent-utils-21: parent-child
created_at: 2026-09-25T04:10:39.786278915+00:00
updated_at: 2026-09-25T11:39:54.964356648+00:00
---

# Description

[gpt-5.6-sol] Add a load-time include mechanism with namespaces to Python and Rust dagrun, then use it to assemble tool-owned validation DAG fragments.

# Acceptance Criteria

The reserved include key is never silently ignored; malformed, absolute, escaping, cyclic, duplicate, or conflicting includes fail explicitly; Python and Rust behavior is equivalent; path-aware loaders and every CLI command accepting `--dag` consume the same flattened graph, while context-free parsers reject includes; nested fragments are exercised by parity tests.
