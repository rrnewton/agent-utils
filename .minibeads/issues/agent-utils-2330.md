---
title: 'agentctl-send-queue-only-headless-wait: send --queue-only to a headless agent waits up to --ready-timeout instead of returning once journaled'
status: open
priority: 2
issue_type: bug
assignee: cl-leanprofile
labels:
- agentctl
- headless
created_at: 2026-11-26T19:40:00-05:00
updated_at: 2026-11-26T19:40:00-05:00
---

# Description

`agentctl send NAME --queue-only TEXT` says it queues the prompt "and return[s] at once", and that "headless and cloud sends already return once journaled". For a headless target that is not true. The send dispatch in `rs/agentctl/src/cli.rs` tests `is_headless(target)` before `queue_only`, so `--queue-only` is ignored for headless agents. The send then waits for `--ready-timeout` (default 900 s), polling until the model starts the input, which can be the end of the agent's current tool call or turn.

A caller that must never block (a coordinator under a short per-call time limit, #2322, or a restore notice) is held for minutes, or is killed by its own time limit after the item was already queued. A caller killed that way cannot tell the send succeeded and is tempted to send again, which duplicates the message.

# Design

In the headless branch of the send dispatch, pass a zero wait to `send_headless` when `--queue-only` is given, so the call returns after the item is journaled and the wrapper is woken. The JSON outcome is then pending, as it already is for interactive `--queue-only`. Keep the existing refusals (`--queue-only` with `--urgent` or `--deadline`). Update the option's help text only if wording changes. Nothing else changes: without `--queue-only`, a headless send still waits for the start as today.

# Acceptance Criteria

- A test sends `--queue-only` to a headless agent whose wrapper is busy (or absent) and returns within a second with the item pending in the queue; the same send without the flag still waits.
- `agentctl send --help` stays accurate for headless targets.

# Notes

Found while designing #2322 agentctl-coordinator-lean-headless-profile (owner request 389), where every coordinator send must finish within about 30 s.
