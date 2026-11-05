---
title: 'watch-status-requests: watch-agents pages the coordinator when a status file raises a request, event-driven'
status: open
priority: 1
issue_type: feature
assignee: cl-start
created_at: 2026-11-05T02:58:17.088723802+00:00
updated_at: 2026-11-05T02:58:17.088723802+00:00
---

# Description

The owner (2026-11-04 21:55 ET, via the other team's lead): idle detection and sweeps should be event-driven, not dependent on an agent remembering to check. On 2026-11-04 a reviewer's verdict sat for 2.5 h. It was written to the reviewer's status file under a heading carrying a "please respond" marker, and a 30-minute aggregate digest buried it as one line among 40.

# Design

Generic, configured per registry (for example in the watcher's config):
1. An inotify watcher on a configured status-file glob (for example `temp/*-status.md`). When a NEW heading carries a configured request marker (for example `COORDINATOR_RESPOND_PLEASE`), deliver ONE durable message per heading to the registry's designated coordinator. Delivery goes through the queue and #844's routing, never into a jammed pane, and the message carries the agent name, path and heading text. Re-page every N minutes (default 30) until the heading is edited to an ack form: it starts with `ANSWERED`, `LANDED` or `[answered:` (configurable).
2. On an agent's working -> idle transition, if its newest status heading is an open request, page the coordinator immediately.
3. Dedupe by (agent, heading hash), and keep a delivery record.

Heading parsing and ack rules: port the rules of the consumer's existing respond-please watcher script (read it in that team's checkout); do not import its code.

# Acceptance Criteria

- Tests:
  - a new marked heading -> one page;
  - an edit to an ack form -> paging stops;
  - a re-page after N minutes (fake clock);
  - working -> idle with an open request -> an immediate page;
  - dedupe;
  - never typed into a jammed pane (#844 routing).
- Config documented in the userguide; gate passes. First deployment: the other team's registry on the lead host.

# Notes

P1, owner request via the lead (2026-11-04). Assigned to cl-start after #852 P0b.
