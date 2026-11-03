---
title: 'chat-delivery-latency-emoji: an opt-in delivered reaction that shows ack-to-delivered minutes'
status: closed
priority: 3
issue_type: feature
assignee: mu-transcript@devbig014
labels:
- agentctl
- chat
depends_on:
  agent-utils-714: blocks
created_at: 2026-11-03T01:37:46.258002759+00:00
updated_at: 2026-11-03T01:37:46.258002759+00:00
closed_at: 2026-11-03T06:44:10.875415+00:00
---

# Description

[opus 5.5] Owner request, filed for the devbig014 coordinator. Make the bridge's SECOND reaction (the "delivered" one) show how long delivery took, so a glance at the channel shows a slow coordinator without opening any records. Opt-in per bridge; off by default. It measures the same ack->delivered stage as #801 chat-latency-summaries.

# Design

- Measure ack->delivered for the request and round it to the nearest minute.
- Under 30 s: keep today's second emoji.
- 1 to 10 minutes: :one: :two: :three: :four: :five: :six: :seven: :eight: :nine: :keycap_ten:.
- Over 10 minutes (10.5 min and up): :hourglass_flowing_sand:.
- A bridge configuration option turns it on, off by default.
- A per-message override (for testing) forces the encoding on one message, so the mapping can be checked without waiting minutes.

# Acceptance Criteria

- With the option off, reactions are exactly as today.
- With it on, an ack->delivered of 20 s shows today's emoji; 31 s and 89 s show :one:; 5 min 20 s shows :five:; 10 min 29 s shows :keycap_ten:; 10 min 30 s shows :hourglass_flowing_sand:. Each boundary is covered by a test.
- The per-message override produces the mapped emoji for a chosen duration.
- The option and the override are documented in the bridge's help and user guide.
