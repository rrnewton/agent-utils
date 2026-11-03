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

[opus 5.5] Agreed with the lead 2026-11-03; implemented in the commit that adds this section.

- **Opt-in.** `validator.offload`: `off` (default and absent: exactly the in-process path), `compare` (in-process validation decides; every validation is also published to donors and their receipts are compared with the local verdict, never landed on), `decide` (donors only; an accepted receipt is the verdict). Rollout runs `compare` first and switches to `decide` only after a run of matching verdicts, so a donor with a subtly different environment can never produce a false green that lands. `run`/`serve --offload` override the setting.
- **Transport, no new ports.** A donor runs this tool on the queue host through a configured route (`donor run --queue 'agentcloudctl node exec --node QUEUEHOST -- merge-queue --repo DIR'`, ssh, or a local path); the route is the authentication. Requests (`donor claim|renew|bundle|report|release`) are short calls printing JSON. The lease token travels on stdin only: never in arguments, logs or `donor status`.
- **Lease store.** `<state>/donor/jobs/<id>.json` (mode 0600, directory 0700) behind its own millisecond lock, so donors never wait on the writer lock a work cycle holds. queued -> leased(donor, random 256-bit token, expiry) -> done(receipt) | cancelled; an expired lease returns the job to queued. One lease per job; donors renew every quarter of `donors.lease_secs` (default 120) and stop when told the job was cancelled or the lease lost; a late receipt is refused. Per-donor concurrency: `donor run --jobs N`, capped by `donors.max_jobs_per_donor`.
- **Candidate tree.** A donor whose clone has the commit uses it; otherwise it fetches its remote for the job's base and asks for a git bundle of the candidate over the base, streamed raw over the route's stdout (node-exec pipes stdio both ways like ssh; measured 512 MiB in 22 s between two hosts). Bundles over `donors.max_bundle_bytes` (default 256 MiB) are refused and the donor declines the job. The donor checks commit and tree before running.
- **Receipt and trust.** Receipts (`merge-queue-receipt/v1`) carry job, donor and host, commit, tree, tool identity, merge-queue version, exit, refusal, per-test results, timing and the log tail. The tool identity is a digest of the validator settings plus the answers of the configured `validator.tool_identity` commands (a named list, e.g. the consumer repository's commit, its pinned tool repository's commit and this suite's commit); a donor whose answers differ declines loudly and backs off. The queue accepts a receipt only for the live lease (token and donor), the job's exact commit and tree, and the identity it computed at publish time.
- **Landing unchanged.** Only the queue host writes to the base, fast-forward to the next green candidate.
- **Queue host as a donor.** In `decide` mode the queue host runs its own `donor run` (local route, its own clone), so local capacity is one donor among several; with offload off nothing changes.

# Acceptance Criteria

- A donor on a second host takes a queued job under a lease, validates the exact candidate SHA, and returns a receipt; the queue lands that candidate on the receipt alone.
- A receipt for a different SHA, tool version or an unauthenticated sender is refused.
- A donor that dies mid-job loses its lease after the timeout and the job runs elsewhere; no job is validated twice concurrently.
- Per-donor concurrency is configurable and enforced.
- The first version is usable by the consumer that requested it; help and user guide cover the donor command and its settings.
