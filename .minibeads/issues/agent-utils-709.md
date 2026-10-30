---
title: 'vt-live-tail-rust: supervise the Slack tail in the Rust server, retire the Python adapter'
status: closed
priority: 1
issue_type: task
created_at: 2026-10-30T20:21:30.647074166+00:00
updated_at: 2026-10-30T21:08:14.656414433+00:00
closed_at: 2026-10-30T21:08:14.656414333+00:00
---

# Description

[opus 5.5] Slug: vt-live-tail-rust.

Owner (2026-10-30): "I thought we were moving towards Rust only across all of our agent-utils projects. Why do we have this Python long-running process?!"

The process is `vibe-talk/scripts/live-hint-adapter.py` (~1800 lines, Python), run on the Slack deployment host as user unit `vibe-talk-slack-hint`: it runs the provider CLI's conversation tail (records mode, #452 slack-live-ingest) for the configured channels and POSTs each record to vibe-talk's `/api/v1/live/tail` (ingest token), with backoff, saved positions, and (since #708 vt-live-push) bridge records arriving separately via agentctl event sinks.

## Wanted

Port it INTO the vibe-talk Rust server (`vibe-talk/src`): the server supervises the tail child process(es) itself (spawn, read JSON records from stdout, apply them through the same code path `/api/v1/live/tail` uses, persist the resume position, restart with backoff, bounded memory, wait-for-ready semantics), configured from app.toml (channels, meta CLI command, identity), so there is no separate Python process or unit. Keep behaviour identical: same record handling, catch-up/resume, the 0.3-1.2 s latency for owner posts; keep `/api/v1/live/tail` for external sources (agentctl sinks). Write a parity test that replays recorded tail output through both the old adapter and the new Rust path and compares the resulting cache/records; then retire the Python adapter (move it out of the deploy, keep it in git history only) and update the unit/deploy notes and README. Server code only; the page (`web/`) is frozen for #624.

Also list, in the handoff, the other long-running Python processes in vibe-talk's deployment (e.g. `scripts/local_tts_sidecar.py`, and the private slack shim outside the repo) with a one-line port plan each; do not port them in this lane.
