---
title: 'gate-flake-triage: raise the full-gate pass rate by triaging steps that fail in full runs and pass alone'
status: open
priority: 1
issue_type: bug
assignee: mu-validate@devbig030
labels:
- validation
created_at: 2026-11-03T19:55:54.120106978+00:00
updated_at: 2026-11-03T19:55:54.120106978+00:00
---

# Description

[opus 5.5] Filed for the devbig014 coordinator (P1). On 2026-11-03 only 24 of 39 full gates passed (62%), counted from that day's gate logs since midnight ET, excluding reruns. Most failures were steps that passed when rerun alone, so lanes paid solo reruns or landed past a red by judgment. Full-gate failures that day: rust.agentctl.test 4; cross.agentctl.differential-2 3; python.agentctl.shard-0 3; vibe-talk.check.keyboard-browser 2; cross.herdr-run.differential 2 (#815); singles: rust.chat-subscription.slack, vibe-talk.check.thread-card-header-browser, vibe-talk.shots.screenshots, vibe-talk.check.desktop-layout-browser, python.agentctl.shard-1, rust-packages.rust.agentctl (and one genuine rustfmt miss). python.wrkslots.shard-3 OOMs whenever selected (#818).

# Design

For each repeat offender decide, with evidence: LOAD (memory or CPU contention inside the full dagrun: compare its profile and timing in failing runs with solo runs) or a REAL RACE (failing case names and order, reproduction under an artificial load). Fix load problems in the scheduler or the step's budget (owner rule: CPU budgets bite, wall clock about 8x, memory caps with headroom per #818); fix races in the test or the product, never with longer sleeps or by skipping. Record each verdict and fix in this issue. Follows #815 and #818.

# Acceptance Criteria

- The full-gate pass rate, measured the same way over a working day after the fixes, is at least 90%.
- Each step listed above has a recorded verdict (load or race) and either a fix or a reason it was left.
- No step is fixed by skipping it or by lengthening a sleep.

# Notes

- [opus 5.5] 2026-11-03 ~18:00 ET: vibe-talk.test.page-0 failed a full gate on the #532 Shift+O latency budget ("a Shift+O took over 50 ms: 88.6, 93.9, 122.9, 49.0") while gating #828, which does not touch that code; it passed in the lane's inner loop. A 50 ms wall-clock budget is the kind of check the owner's timeout rule says must not turn red under contention.
## Triage (mu-validate, 2026-11-03)

### rust.agentctl.test — REAL BUG (fixed)

[muse watermelon-full-preview] Failing cases `chat_reply::a_reply_id_alone_finds_the_bridge_and_its_request_through_the_index` and `chat_reply::a_bridge_targeting_the_caller_is_chosen_and_an_ambiguous_one_is_refused` (gate-742c and others) were not load: the reply ID is `{base64url nonce}_{ordinal}` (`current_reply_id` in `rs/agentctl/src/chat_runtime.rs`), and a nonce starting with `-` (probability ~1/64 per ID) made clap refuse the positional ID as an unknown option (`unexpected argument '-3' found`, exit 2) in exactly the `agentctl chat reply ID` form a prompt prints. Fix: `allow_hyphen_values` on the positional reply ID of `chat reply`, `chat sent`, `chat thread`, and `chat help` in `rs/agentctl/src/cli.rs`, with a parse regression test (`chat_reply_id_may_start_with_a_hyphen`). This also explains part of the `cross.agentctl.differential-*` and `python.agentctl.shard-*` flakiness whenever their cases mint reply IDs. `directory::rejection_lookup_and_write_share_one_deadline_even_through_the_warm_wrapper` also failed in gate-742c; verdict pending (deadline-shaped, likely LOAD).
