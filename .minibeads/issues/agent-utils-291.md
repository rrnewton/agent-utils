---
title: 'stop-lost-wakeup: make a chat bridge stop wake every waiter, and exit 0 when a stop cancels a Herdr call'
status: open
priority: 2
issue_type: bug
labels:
- agentctl
- chat
created_at: 2026-10-14T14:48:59.066608955+00:00
updated_at: 2026-10-14T14:48:59.066608955+00:00
---

# Description

[opus 5.5] Found by the 2026-10-14 branch triage (ai_docs/transient/2026-10-14-branch-triage/REPORT.md). Prior work: commit 9d21d12e on archived/chat-bridge/185-186-wip-7 (written for another tracker's #185 stop-lost-wakeup and #186 stop-cancelled-herdr-call); it no longer applies and must be redone by hand. On main, in rs/agentctl/src/chat_service.rs: (1) StopState::stop stores the flag and calls notify_all without taking self.lock, while StopState::wait checks is_stopped under the lock and then waits, so a stop that lands between the check and the wait is lost; the provider reconnect backoff waits up to 60 s there. (2) run_owner_loop calls pane_info_with_runtime right after active.wait returns, with no stop check, and a Herdr call cancelled by the stop falls through as an error, so `agentctl chat run` exits 1 on an ordinary stop. Fix: take the lock in stop(); check stop.is_stopped() after the wait; add a cancelled AdapterErrorKind, set where client.rs and subagents.rs build the 'cancelled' errors, and treat it as a clean stop; port the branch's tests and tighten run_owner_loop_timed, which currently accepts a cancelled error after a stop. Also carry 14a821c4's two-sentence correction to embedded_chat_userguide.md. These are chat-owned files: coordinate with their owner.

# Acceptance Criteria

A stop is never lost between the check and the wait (test); a stop during a Herdr call ends chat run with exit 0 (test); the guide wording matches the code.
