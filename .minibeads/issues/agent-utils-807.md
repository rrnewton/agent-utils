---
title: 'merge-queue-donor-validators: other hosts validate queued candidates and return exact-SHA receipts'
status: open
priority: 1
issue_type: feature
assignee: cl-mq@devbig030
labels:
- merge-queue
created_at: 2026-11-03T05:59:49.771324777+00:00
updated_at: 2026-11-03T05:59:49.771324777+00:00
---

# Description

[opus 5.5] Owner request, filed for the devbig014 coordinator (P1). The local merge queue validates every candidate on the coordinator host, which caps throughput at that host's few concurrent full validates: one consumer is draining 129 PRs tonight with a local cap of 2. Other hosts sit idle. Donor validators let other hosts take queued validate jobs, run them with the same validate tooling, and return exact-SHA receipts the queue accepts as landing evidence, so validation scales across machines while landing stays on one host.

# Design

- Donor daemons on other hosts pull queued, valuable validate jobs, run them locally with the same validate tooling, and report receipts.
- A receipt binds the candidate SHA, the tool identity and version, the host and the verdict, and is authenticated (agent-teams sender proof or a node-exec'd command; no new open ports).
- The coordinator host stays the only writer to main: fast-forward only, to the next green candidate.
- One lease per job: a job never runs twice at once, and an abandoned job is re-queued after a timeout.
- Bounded concurrency per donor (configurable); donors honour the timeout rule (CPU-time budgets, wall clock about 8x the CPU budget).
- Before building, the owner of this issue sends a short design: transport, receipt format and trust, lease store, and how a donor gets the candidate tree.

# Acceptance Criteria

- A donor on a second host takes a queued job under a lease, validates the exact candidate SHA, and returns a receipt; the queue lands that candidate on the receipt alone.
- A receipt for a different SHA, tool version or an unauthenticated sender is refused.
- A donor that dies mid-job loses its lease after the timeout and the job runs elsewhere; no job is validated twice concurrently.
- Per-donor concurrency is configurable and enforced.
- The first version is usable by the consumer that requested it; help and user guide cover the donor command and its settings.
