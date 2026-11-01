---
title: 'validate-speed: reduce gate work while preserving observed coverage'
status: closed
priority: 1
issue_type: task
assignee: mu-validate
created_at: 2026-11-01T15:57:04.657241243+00:00
updated_at: 2026-11-01T15:57:04.657241243+00:00
---

# Description

[gpt-6] Measure gate profiles and case timings; remove repeated work; add fail-closed selection within components; make contention-sensitive tests deterministic. Preserve the complete validation DAG contract and record page, Rust-tool, and notes before/after results.

[muse watermelon] Closed by mu-validate: delivered in 4bd9f9e7, 5786f0e6, 8e2c2114, 9d296e04 on main. Final gate: PARTIAL with every selected node passed (215 PASS, 0 FAIL; 7 delegated-live-scope cross skips unverified). See ai_docs/transient/2026-11-01-validate-speed.md.

[muse watermelon] Follow-up under #725 validate-speed (not a new issue): gate admission landed in 1f97c37e (scripts/gate.py, 3 shared slots + exclusive class). The earlier local reference to a separate admission issue number was a mis-allocation and is removed; pushed history is unchanged.
