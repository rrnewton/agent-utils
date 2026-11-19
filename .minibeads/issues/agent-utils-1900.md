---
title: 'agentctl-services-repair-watch-first-pass-health: repair watch fails turnover health on a large registry'
status: open
priority: 1
issue_type: task
labels:
- agentctl
created_at: 2026-11-19T04:44:01-05:00
updated_at: 2026-11-19T04:44:01-05:00
---

# Description

[watermelon-full-preview] A services turnover keeps failing its
health check on the repair watch (the `agentctl watch` daemon,
discovered as kind `watch`): its first pass over a large registry
takes longer than the flat 120 s health window, so a healthy new
process is rolled back. `health_timeout` in
`rs/agentctl/src/services/refresh.rs` extends the window only for
kind `watch-agents` (its pass budget plus a soak and a margin);
kind `watch` gets the flat timeout, and both kinds must complete a
whole pass (`health.json` / `agent-watch.json` with a `checked_at`
after the restart) before health passes. The unit was held with a
turnover pin to stop the churn.

# Design

Classify every long-pass watcher kind for the scaled health window
(`watch` as well as `watch-agents`, from its pass budget or interval
with the same fallback), and prove health by progress instead of
only by a completed pass: each watch daemon writes a new
`pass-progress.json` in its state directory at the start of a pass
(`pass_started_at`) and keeps its `heartbeat_at` advancing while it
runs. Services health accepts a pass that started after the restart
with a fresh, advancing heartbeat from the same process (which must
still stay up for the soak), and still accepts a completed pass as
before. A unit that never starts, never begins a pass, or crashes
still fails health and rolls back. The new file is additive: older
builds ignore it, and readers of `health.json` and
`agent-watch.json` are unchanged.

# Acceptance Criteria

- A test with a large fake registry whose first pass exceeds 120 s
  on the fake clock refreshes the repair watch instead of rolling
  it back (it fails before the fix).
- The same progress proof works for `watch-agents`, and a completed
  pass still proves health for units whose daemon writes no
  progress file.
- A watcher that crashes on the new binary, or never begins a pass,
  still fails health and is rolled back.

# Notes

[watermelon-full-preview] Related: #701 agentctl-services-refresh
(the health check this extends), #1890
agentctl-turnover-watch-health-window (scales the `watch` window
and continues turnover past rollbacks; this issue adds the
progress proof, and whichever lands second rebases over the other),
#1225 workspace-goal-reconciler (the other `watch` daemon work).
Services and both watch daemons are Rust-only, so there is no
Python mirror. The new persisted file was reported to the
coordinator before gating, as reader-skew rules require.
