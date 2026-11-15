---
title: 'summarizer-loud-failure-and-fallback-chain: loud summariser failures and per-kind fallback model chains'
status: open
priority: 1
issue_type: feature
labels:
- vibe-talk
created_at: 2026-11-15T08:04:54-05:00
updated_at: 2026-11-15T08:04:54-05:00
---

# Description

Owner request 092 (vibe-talk server and page health): "seems like we need
that to fail more loudly so we can switch over or fall back to some other
model". Thread names were missing on the live instance; the cause was
Slack rate-limit starvation of the naming read, not the model, but
summaries and thread naming failing for any reason (model error or
withdrawn model, gateway errors, timeouts, rate-limit starvation, empty
answers) must surface loudly, and each summary kind gets an ordered
fallback model chain to switch to automatically.

# Design

- **Config.** Each `[summaries.llm.models]` value (`message`,
  `thread_name`, `digest`, `side_agent`) may be a single string (a
  one-element chain, as before) or an ordered list, e.g.
  `["small-elk-internal", "muse-spark-1.4-internal"]`.
  `[summaries.llm] fallback_cooldown_seconds` (default 300) is how long a
  failed model is skipped before it is retried.
- **Fallback.** Per kind, a request tries the chain in order, skipping
  cooled models; any failure (refusal, transport, unusable answer,
  withdrawn-model answer, timeout, rate limit) cools that model, logs the
  switch (`summarizer_fallback`), and retries the same request on the
  next model. After the cool-down the primary is retried.
- **Loud thresholds.** A kind goes loud at 3 consecutive all-chain
  failures, or at a failure rate of at least half over the last 20
  attempts with at least 5 attempts; 2 consecutive successes clear it.
  One blip never warns. Loud means an ERROR log under the stable target
  `summarizer_health`, the `summarizer_health` section of
  `GET /api/v1/diagnostics`, and one persistent page banner naming the
  model in use and why. The health state keeps the reason class (model
  or gateway, upstream starvation, timeout) so the headline never sends
  the owner chasing the model when the cause is upstream starvation.
  Thread-naming starvation counters (`thread_naming`: unnamed due,
  overdue unnamed, paused channels, consecutive pauses; loud over one
  hour unnamed or 3 consecutive pauses) fold into the same report and
  banner — one health surface, agreed with the naming work.
- **Cache rule.** The summary policy version hashes the CONFIGURED
  chain, never the runtime active model, so a fallback switch keeps
  every cached summary reachable; only editing the chain in
  configuration re-derives. Thread names stay under the kept version,
  unaffected.

# Acceptance Criteria

- Fake-gateway unit tests: a primary error switches to the next model
  with the switch logged; a withdrawn-model answer switches; the
  primary is retried after the cool-down; all models failing errors and
  goes loud only past the threshold; one blip gives no warning; a
  single-string config still reads as a one-element chain.
- Browser check: the page health banner shows with the model name at
  phone (about 390 px) and desktop widths, with screenshots looked at.
- Diagnostics carries `summarizer_health` (overall, headline, per-kind
  state, `thread_naming`), and `/healthz` carries overall plus headline
  so a loopback deploy watch can grep it without a token.

# Notes

Coordinated with the naming-starvation health work: one
`summarizer_health` object and one page banner, counters folded in, no
second surface. Live chain configuration is set by the coordinator in
the host overlay after landing; this change never edits live config.
No changes to `vibe-talk/web/voice.js` or `web/src/app/main.js`.
