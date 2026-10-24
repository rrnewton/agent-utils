---
title: 'agentctl-team-messaging: message another team''s coordinator across hosts, checking stale state as it goes'
status: closed
priority: 2
issue_type: feature
labels:
- agentctl
- design
created_at: 2026-10-24T13:05:22.511039550+00:00
updated_at: 2026-10-24T13:24:06.161714708+00:00
closed_at: 2026-10-24T13:24:06.161714578+00:00
---

# Description

[opus 5.5] The owner asked for a design of an agentctl extension for inter-team messaging alongside ordinary chat messages. A consuming project's agents are the primary users. Not a live mirror of local Herdr state between machines: a sparse model of which hosts run which teams and where each coordinator entry point lives, with extra checking at send time because that model is likely to be out of date. Design record: ai_docs/transient/2026-10-24-agentctl-team-messaging.md.

# Acceptance Criteria

A design record covering a teams registry with discovery, team send with attribution, idempotency, pre- and post-delivery verification, durable outbox and receipts, expiry and superseding for idle-only targets, safety and scoping, chat bridges as contacts, migration from an existing per-host coordinators file, CLI and file sketches, a phased plan and owner questions. Implementation is follow-up work.
