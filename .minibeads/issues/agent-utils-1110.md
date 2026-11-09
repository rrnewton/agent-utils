---
title: 'test-deep-path-sockets: Rust tests that bind sockets under the checkout fail in deep worktrees'
status: open
priority: 2
issue_type: bug
assignee: mu-docreader
created_at: 2026-11-09T03:04:42.000000000+00:00
updated_at: 2026-11-09T03:04:42.000000000+00:00
---

# Description

Found gating #1081 documents-relative-links from a nested subagent worktree (path prefix 161 bytes: `worktrees/slots/<slot>/.muse/worktrees/subagent-v2-<uuid>-<uuid>/`). Two Rust suites fail there and pass from a shallow checkout of the identical tree, because they place filesystem paths under the checkout/test working directory:

- `rust.agentctl.test`: thread 'chat_service::tests::the_wake_socket_is_named_by_the_absolute_path_that_a_prompt_prints' panicked at agentctl/src/chat_service.rs:19222:14: "a working directory shorter than 95 bytes" (1 of 2,422 cases). The wake socket's absolute path is bounded by the 107-byte socket-address limit, so any checkout deeper than ~95 bytes breaks this test by construction.
- `rust.chat-subscription.slack`: rescan tests (12 threads) panicked with `slack_configuration_invalid`: "diagnostics_log must be a nonempty single-line string of at most 256 bytes" — the test-built diagnostics_log path under the deep worktree exceeds the product's own 256-byte cap.

Both are real test bugs, not load: they fail deterministically in a deep checkout and pass in a shallow one (verified 2026-11-08: deep run red, shallow `gate.py --rerun` of the same tree green, 2,422/2,422 and 178/178). Nested worktrees are now the normal shape for subagent work, so any lane gating from one reads these as its own failures.

# Design

- Tests that bind Unix sockets or build config paths with byte caps should root them in a short temp dir (e.g. the system temp dir with a short prefix, as the chat wake socket itself already does at runtime under /dev/shm) instead of the checkout-relative working directory; where a test's subject IS the working-directory path (the wake-socket naming test), create the short working directory explicitly rather than asserting the harness provided one.
- Audit for siblings: other tests asserting on cwd length, socket paths under target/, or config path byte caps, in agentctl and chat-subscription crates first.

# Acceptance Criteria

- `rust.agentctl.test` and `rust.chat-subscription.slack` pass from a checkout nested at 160+ bytes (the subagent worktree shape), not only from shallow slots.
- No other gate step's verdict changes.

# Notes

[watermelon-full-preview] mu-docreader, thread 214. Found while gating #1081 documents-relative-links; the shallow-rerun evidence is in that landing's report.
