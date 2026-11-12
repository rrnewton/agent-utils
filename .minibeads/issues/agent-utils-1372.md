---
title: 'chat-stale-prompt-wedges-inbox: a chat prompt pending past a cap delivers while working or alarms as wedged; status reports its age'
status: closed
priority: 1
issue_type: bug
labels:
- agentctl
- chat
created_at: 2026-11-12T13:30:00.000000000+00:00
updated_at: 2026-11-12T18:00:00.000000000+00:00
closed_at: 2026-11-12T18:00:00.000000000+00:00
---

# Description

On 2026-11-11 one undelivered chat-bridge prompt (a post-incident restore
"I'm back" prompt) sat in a coordinator's inbox from 3:09 PM to 9:04 PM ET.
The bridge types only when the agent is idle and the coordinator was busy.
Senders that refuse to type ahead of a pending chat prompt deferred every
delivery behind it: 243 alarm deliveries, median wait 119 min, max 395 min.
An earlier post-restore prompt did the same 21 times; it repeats after every
restore.

Related prior work: #1158 coordinator-never-blocks (landed in series
323fcc6b: supersede keys, self-expiring notices, blocked-coordinator owner
page), #1156 queue-supersede-key, #1157 automated-notice-ttl, #1020
agentctl-rebind-queue-target (status names a mismatch and pending ages),
and the chat delivery issues #556 chat-delivery-latency, #697
send-muse-while-working, #698 chat-requests-unbatched (deliver-while-working,
owner prompts first and never batched).

# Acceptance Criteria

- A chat prompt pending longer than a configurable cap (default 10 min) is
  delivered while the agent works, with the bridge's read-before-typing check
  applied at that moment, or escalated to the owner alarm path saying the
  coordinator inbox is wedged by a chat prompt aged N min; a fresh prompt is
  untouched.
- `agentctl status`/queue output exposes the oldest pending chat prompt's
  age, and the userguide documents the sender contract.
- Post-restore prompts carry a supersede key so a new boot replaces an
  undelivered earlier one instead of re-creating the wedge.
- Tests use fake clocks or conditions, with no wall-clock budgets: a stale
  prompt with a busy agent is delivered or alarmed; a fresh prompt is
  untouched; the age is reported; a restore prompt is superseded, not
  duplicated.

# Notes

Rust edition is primary; the Python edition stays consistent where it has
the path (queue status). No consumer names in files.

## Resolution

[watermelon-full-preview] Landed on main as c9282c32 (feature), 4f7132f6 and
65cfe8ba (validation classification), 97684d2f (gate fixes + this record's
evidence trailer). chat run --stale-prompt-cap (default 10m) delivers a stale
prompt while the agent works or alarms it as wedged; status reports
oldest_pending_chat_age_seconds; restore prompts carry supersede keys.
Validation: touched checks green; cross herdr-agent differential 5
consecutive solo passes (its 10s lock case is a wall-clock test defect under
load, mu-crossrobust de-timing); untouched dagrun/vibe/timeline reds passed
solo. Deployment of live bridges is the coordinator's step.
