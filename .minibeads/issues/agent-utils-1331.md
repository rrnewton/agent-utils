---
title: 'claude-session-summarizer: a warm persistent Claude session as a vibe-talk Summarizer backend'
status: open
priority: 1
issue_type: feature
labels:
- vibe-talk
created_at: 2026-11-11T20:00:00+00:00
updated_at: 2026-11-11T20:00:00+00:00
---

# Description

[watermelon-full-preview] Owner request (reply 390, normal-to-high priority, not a
UI fast-track): if the Rust summarizer already has clean traits, plug a persistent
Claude session in as an alternate summarizer, and keep one started up so a summary
does not always pay a fresh startup. The owner asked how Haiku latency compares
when the session stays warm.

Liaison measurement (devbig014; `claude -p`, claude-haiku-5-5, `--tools ""`,
`--no-session-persistence`, AI gateway): cold per call 11.8 / 14.2 s; a
persistent stream-json session: first turn 11.4 s, then 2.1 / 1.2 / 1.3 s (first
text 0.8-1.5 s). Script: `/tmp/haiku-lat-scripts/warm.py <model>`.

# Design

## Crate boundary

- Factor agentctl's headless stream-json driver (`rs/agentctl`, the
  run-headless path) into a shared crate that both agentctl and vibe-talk use,
  with no behaviour change for agentctl (its tests stay green). This refactor
  lands first, as its own gated landing, before any backend work.
- The shared crate owns only the transport: spawn, stream-json framing,
  turn correlation, and shutdown. It knows nothing about summaries, pools,
  or vibe-talk config.

## Backend and pool lifecycle

- A `claude_session` backend implements the vibe-talk `Summarizer` trait like
  the other backends, with tools off, and plugs into `summarizer_for` and into
  the per-kind model map from #1324 per-kind-summarizer-model
  (`[summaries.llm.models] <kind>`), so a kind can pick the `claude_session`
  backend. Usable by Discuss (#1316 catchup-side-agent, lane mu-catchup).
- A warm pool of 1-2 sessions, started eagerly so the first summary does not
  pay cold start. Sessions are recycled after N turns or K tokens (config),
  restarted on death, and requests are queued fairly across the pool.
- Model and effort come from config (default claude-haiku-5-5).

## Failure modes

- A session that dies mid-turn fails that request by name and is restarted;
  it does not poison the pool or hang the queue.
- A deployment with no usable Claude CLI or gateway fails per request with
  the existing `SummaryError` shapes, never silently falling back to another
  backend's wording.
- Pool exhaustion queues fairly rather than spawning unbounded sessions.

## Config keys

- Backend selection per kind via #1324's `[summaries.llm.models]` map, with
  the `claude_session` backend named there.
- Pool size (1-2), recycle thresholds (N turns, K tokens), model
  (default claude-haiku-5-5) and effort, all under the summaries config.
  Exact key names are settled in the design note on this issue before code.

# Acceptance Criteria

- The shared-crate refactor lands alone with agentctl's tests green and no
  behaviour change for agentctl.
- The `claude_session` backend serves message summaries and thread naming
  through the `Summarizer` trait, selectable per kind through #1324's map.
- Pool tests cover: warm reuse across turns, recycle after N turns / K
  tokens, restart on death, and fair queueing. No wall-clock budgets in
  tests (owner rule 346).
- A latency A/B (and a quick quality read) against Spark 1.4 and
  small-elk-internal on mu-elk's task set, reusing mu-elk's inputs and
  harness rather than duplicating them; real model calls kept affordable.
- The A/B is reported as a harness doc in `ai_docs/reports/measurements/`
  if that directory exists by then, else `ai_docs/`, with a CHANGELOG
  bullet.
- The ElevenLabs agent backend stays maintained and first-class (owner
  reply 388); it is not regressed or described as legacy.

# Notes

Related: #1324 per-kind-summarizer-model (mu-elk, lands first; this work
rebases on it), #1316 catchup-side-agent (Discuss), #1027
vibe-talk-spark-summarizer, #233 llm-summarizer. Implementation sequences
after #1324: shared-crate refactor first as its own gated landing, then
the backend.
