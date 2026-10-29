---
title: 'chat-bridge-repoint: retarget a running chat bridge to another agent or pane live, with health checks'
status: open
priority: 1
issue_type: feature
labels:
- agentctl
- chat
created_at: 2026-10-29T11:59:00.940749556+00:00
updated_at: 2026-10-29T11:59:00.940749556+00:00
---

# Description

[opus 5.5] Owner request (2026-10-29): agentctl needs a fast way to switch which agent / Herdr pane a running chat bridge talks to, without bringing its systemd unit down, plus a quick way to tell whether a bridge is healthy. Any agent on the host should be able to repoint a bridge, so the owner can recover when a coordinator dies or must be replaced.

Wanted:
- chat repoint --bridge-state DIR --agent NAME [--pane ID]: takes effect on the running bridge (control socket or watched state file), validates the target first (registered, pane alive, harness detected), records who repointed it and when, and posts a short notice in the channel.
- chat health --bridge-state DIR: one-line verdict plus details: process alive, event stream cursor fresh, last admitted / delivered / replied times, target agent and pane alive, stuck or uncertain requests, retry loops.
- chat latency --bridge-state DIR [--since T]: per-request admitted -> delivered (first reaction) -> sent-to-LLM (second reaction) -> first reply, from the bridge's own records (Slack's reactions API carries no timestamps).
- Exactly one bridge per channel: refuse to start a second bridge on a channel that already has a live one (seen: a failed and a live unit on one channel, and a bridge typing into the wrong pane).
- Requests already admitted but undelivered move to the new target; uncertain ones are reported, never silently dropped.
Acceptance: repoint a live test bridge between two agents with no restart, with message round trips before and after, and health/latency outputs shown.
