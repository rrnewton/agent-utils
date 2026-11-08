---
title: 'chat-launch-outer-kill-leak: the outer-kill chat launch test can leave a chat run alive for days'
status: open
priority: 2
issue_type: bug
created_at: 2026-11-08T13:40:00.000000000+00:00
updated_at: 2026-11-08T13:40:00.000000000+00:00
---

# Description

On 2026-11-08 a slot could not be removed: its disk image would not unmount ("Device or resource busy"). The holder was a 2-day-old `agentctl chat run` process, re-parented to init, running the slot's own build (`<slot>/rs/target/validation-adapters/agentctl/release/agentctl --registry /tmp/pytest-of-<user>/pytest-26084/test_outer_fixture_sleeps0/registry ... chat run --bridge-state .../chat-state`). It ignored SIGTERM for over 10 s and had to be SIGKILLed.

`test_outer_fixture_sleeps` is the inner helper of `test_killing_outer_pytest_leaves_no_chat_run` in py/tests/test_agentctl_chat_launch_rust.py (#857): an outer pytest launches a fixture whose bridge is a real `chat run`, the test SIGKILLs the outer pytest and asserts the bridge, harness and launcher all die within 30 s. In that run the bridge survived. The test's `finally` block only kills the outer pytest; when the `gone()` assertion fails, nothing kills the surviving fixture processes, so a failing run leaks them indefinitely, holding the slot's mount, its build output and the pytest temp tree.

# Design

- Teardown: on any exit from the test (pass or fail), kill whatever of `record`'s bridge, harness and launcher PIDs still exist (verify each by /proc start time or cmdline before killing, so a reused PID is never hit), then assert.
- Find out why the bridge outlived the outer kill in that run (the #857 mechanism: process group / parent-death signal / launcher supervision) and fix the gap, so the assertion itself holds under load.
- Find out why `chat run` ignored SIGTERM for over 10 s in that state (a bridge whose launcher is gone should stop promptly on SIGTERM).

# Acceptance Criteria

- A run in which the fixture survives the outer kill (forced, e.g. by making the parent-death path a no-op in a test-only hook) still leaves no process behind after the test.
- `test_killing_outer_pytest_leaves_no_chat_run` passes 20 consecutive runs under load with no surviving process.
- An orphaned `chat run` whose launcher is gone exits within a few seconds of SIGTERM, with a test.

# Notes

Found by the devbig030 coordinator while removing a stale slot; filed at the lead's request. Give it to whichever lane next touches py/tests/test_agentctl_chat_launch_rust.py or the chat launcher. Related: #857 (closed), the tmux test-server leak check in validation.
