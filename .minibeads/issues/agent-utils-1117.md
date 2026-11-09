---
title: 'herdr-empty-stdout-actions: treat a zero-exit empty-stdout herdr action as success in both editions'
status: open
priority: 2
issue_type: bug
assignee: mu-rsctl
created_at: 2026-11-09T05:53:08.000000000+00:00
updated_at: 2026-11-09T05:53:08.000000000+00:00
---

# Description

Some successful Herdr action calls exit 0 with empty stdout (observed live: `herdr pane send-text <pane> <text>`). Both editions' Herdr wrappers demand a JSON body and report the success as a failure: Python `py/agentctl/foreign/lib.py` `_herdr` runs `json.loads("")`, which raises and becomes `AgentOperationError("herdr_api_invalid", "... returned invalid API JSON: ")`; Rust `rs/agentctl/src/foreign/lifecycle.rs` `herdr_call` produces the same `herdr_api_invalid` shape.

User-visible consequence, confirmed in a live scratch-workspace keeper exercise on 2026-11-08 (both editions): `herdr-subagents keeper ensure` prints "[keeper] created <tab> pane=<pane>, but could NOT start the status loop: herdr pane send-text ... returned invalid API JSON:" followed by "[keeper] the keeper still protects tab-close; the loop is cosmetic" — even though the text WAS delivered and the status loop is running (verified by reading the pane: status output repeats on the interval; one iteration even GC-reaped a dead scratch agent). The keeper deliberately degrades loudly instead of failing, so the visible damage there is a wrong, alarming message on every ensure; any other caller of an empty-stdout action gets a spurious `herdr_api_invalid` for a call that worked.

# Design

- In each edition's Herdr wrapper used for action calls, treat a zero-exit, empty-stdout response as an empty result (`{}`) instead of invalid JSON. Non-empty invalid JSON stays `herdr_api_invalid`; non-zero exits stay errors.
- Python: in `_herdr` (or a dedicated action helper), empty stdout after a zero exit parses as `{}`.
- Rust: in `herdr_call`, empty stdout after a zero exit yields an empty object, or route pure action calls (`pane send-text` and friends) through `herdr_action` semantics.
- Fix both editions together so parity does not drift.

# Acceptance Criteria

- A test in EACH edition: a fake `herdr` that exits 0 printing nothing for an action subcommand is accepted by the wrapper; a fake printing garbage still raises `herdr_api_invalid`.
- `keeper ensure` against a live (or faithfully faked) Herdr no longer prints the "could NOT start the status loop" warning on a successful send.

# Notes

Found during the B3-7 live-Herdr keeper exercise (scratch workspace, since removed). Parity between editions is currently exact, wart included. Callers audited at filing time: keeper `ensure` is the visible one; any action-style call routed through the JSON-demanding wrapper is in scope. First filed as a GitHub issue (since closed as renumbered); the file ledger number here is authoritative.
