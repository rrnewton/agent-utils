---
title: 'delivery-agent-check: a second delivery check when an agent bridge picks the message up'
status: open
priority: 2
issue_type: task
external_ref: gh-292
created_at: 2026-11-15T17:47:04.000000000+00:00
updated_at: 2026-11-15T17:47:04.000000000+00:00
---

# Description

Today the two vibe-talk send ticks mean the vibe-talk server accepted the send and the upstream chat service settled it (#621 send-double-check, closed). Wanted instead: one small check once the upstream service has accepted the post, and a second once an agent bridge has picked the message up. That pickup signal already exists outside the page (the bridge's check reaction on the upstream message, or bridge receipts). Unconfirmed sends keep today's manual retry with history checked first. Backlog, not urgent, per the report.

# Design

Extend the outgoing-row delivery states in vibe-talk/web/src (send/outbox and row rendering) with a pickup state read from the message's reactions or from bridge receipts via the server. Keep the existing accepted/upstream timing and retry semantics unchanged.

# Acceptance Criteria

- An outgoing row shows the upstream-accepted check, then a distinct second check on bridge pickup; no pickup leaves one check.
- Unconfirmed sends still offer manual retry after a history check.
- Page tests plus a browser check cover both states.

# Notes

[watermelon-full-preview] Filed from GitHub #292 delivery-agent-check (https://github.com/rrnewton/agent-utils/issues/292) during Request 098 triage. Overlaps closed #621 send-double-check but is not it: that pair is server/upstream, this is upstream/agent-pickup.
