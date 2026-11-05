---
title: 'watch-status-requests: watch-agents pages the coordinator when a status file raises a request, event-driven'
status: closed
priority: 1
issue_type: feature
assignee: cl-start
created_at: 2026-11-05T02:58:17.088723802+00:00
updated_at: 2026-11-05T08:27:33.000000000+00:00
---

# Description

The owner (2026-11-04 21:55 ET, via the other team's lead): idle detection and sweeps should be event-driven, not dependent on an agent remembering to check. On 2026-11-04 a reviewer's verdict sat for 2.5 h. It was written to the reviewer's status file under a heading carrying a "please respond" marker, and a 30-minute aggregate digest buried it as one line among 40.

# Design

Generic, configured per registry (for example in the watcher's config):
1. An inotify watcher on a configured status-file glob (for example `temp/*-status.md`). When a NEW heading carries a configured request marker (for example `COORDINATOR_RESPOND_PLEASE`), deliver ONE durable message per heading to the registry's designated coordinator. Delivery goes through the queue and #844's routing, never into a jammed pane, and the message carries the agent name, path and heading text. Re-page every N minutes (default 30) until the heading is edited to an ack form: it starts with `ANSWERED`, `LANDED` or `[answered:` (configurable).
2. On an agent's working -> idle transition, if its newest status heading is an open request, page the coordinator immediately.
3. Dedupe by (agent, heading hash), and keep a delivery record.

Heading parsing and ack rules: port the rules of the consumer's existing respond-please watcher script (read it in that team's checkout); do not import its code.
4. (Owner addition, 2026-11-04 22:19 ET.) When the designated coordinator runs `agentctl send <lane>`, the output ends with the open status-file requests still waiting, one entry each (lane, age, heading excerpt), for example "3 more waiting: A 12m (...), B 4m (...)", or "0 waiting". A new `agentctl requests` command prints the same list. Same parser and ack rules as the watcher. Each reply must remind the coordinator of the remaining requests BY NAME, never as a bare count.

# Acceptance Criteria

- Tests:
  - a new marked heading -> one page;
  - an edit to an ack form -> paging stops;
  - a re-page after N minutes (fake clock);
  - working -> idle with an open request -> an immediate page;
  - dedupe;
  - never typed into a jammed pane (#844 routing).
- `agentctl send` (as coordinator) ends with the named waiting list, and "0 waiting" when empty; `agentctl requests` prints the same list; tests for both.
- Config documented in the userguide; gate passes. First deployment: the other team's registry on the lead host.

# Notes

P1, owner request via the lead (2026-11-04). Assigned to cl-start after #852 P0b.

## Resolution

Delivered by cl-start in rs/agentctl/src/status_requests.rs, with its use in watch-agents, `agentctl send` and the new `agentctl requests`.

- Configuration: REGISTRY/status-requests.json (`status_files` with `{agent}`, `marker`, `ack_words`, `bracketed_ack_words`, `repage_minutes`), documented in the userguide under "Requests in agents' status files".
- Parser: the heading, answer and age rules of the reference watcher, ported (no tz crate: US daylight saving time is computed by hand, with first-occurrence semantics for ambiguous times). A local differential run against the reference script over 1264 status files agreed on every request, age, source and line. The only difference is that Rust marks a truncated heading with an ellipsis.
- watch-agents: one durable page per (agent, heading hash) to the designated coordinator, routed past jammed or held agents (#844), else to the owner alarm. It pages again every `repage_minutes` until the heading is answered, and at once on working to idle. An inotify watch on the status directory pages between passes. State: `status_pages`; report: `status_requests`.
- `agentctl send` from the designated coordinator ends with "N more waiting: lane 12m (heading), ..." or "0 waiting" on standard error, and `waiting_requests` in its JSON. `agentctl requests [--json]` prints the same list.
- Tests: unit tests (parser, paging with a fake clock, jam routing, file-change paging, idle transition, crash-safe message IDs, inotify) and tests/status_requests.rs (send and requests through the binary).

Not done here: the first deployment, on the lead host's other registry, belongs to the lead's coordinator.
