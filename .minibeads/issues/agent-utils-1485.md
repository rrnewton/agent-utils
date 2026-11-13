---
title: 'agentctl-wake-socket-test-cwd-length: wake-socket test makes its own short temp dir'
status: closed
priority: 2
issue_type: bug
created_at: 2026-11-13T11:24:44.717795802+00:00
updated_at: 2026-11-13T13:37:08.579438648+00:00
closed_at: 2026-11-13T13:37:08.579438528+00:00
---

# Description

rs agentctl chat_service::tests::the_wake_socket_is_named_by_the_absolute_path_that_a_prompt_prints panics 'a working directory shorter than 95 bytes' (chat_service.rs) when the checkout path is long, e.g. a nested worktree .muse/worktrees/<long name>. The relative-name part measures a name against the process working directory, so a long checkout leaves no relative name that fits and the test cannot run there.

# Design

The test creates its own short temporary directory (under the system temp dir, as wake_root already does), enters it for the relative-name checks, and restores the saved working directory afterwards. No product change; the socket naming rule is unchanged.

# Acceptance Criteria

The named test passes when run from a checkout whose path exceeds 95 bytes (the nested queuedeliver worktree), and still passes from a short path. No other chat_service test changes behaviour.

# Notes

Seen in the 46e6a929 --all follow-up from a long nested-worktree checkout. Warm-up lane for the queue-delivery work (#1479, #1480).

## Resolution

Landed as afb52e98. The test enters a short temporary directory it creates for the relative-name checks and restores the working directory afterwards; verified passing from the long nested-worktree checkout and in the gated validation (only waivable cross-differential CPU-timeouts red).
