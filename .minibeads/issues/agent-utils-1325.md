---
title: 'discuss-on-agent-backend: Discuss needs an agent-side summary path or a clear disabled state'
status: open
priority: 2
issue_type: feature
created_at: 2026-11-11T19:30:00.000000000+00:00
updated_at: 2026-11-11T19:30:00.000000000+00:00
---

# Description

Discuss (#1316) is built on the direct-LLM summarizer configuration
(`[summaries.llm]`, per-kind models in #1324 per-kind-summarizer-model). On a
deployment running the ElevenLabs agent summarizer backend
(`summarize/agent.rs`) — maintained and first-class (owner reply 388; the
owner's external OSS deployments) — there is no `[summaries.llm]` section, so
Discuss has no model to call. That must be a clear "not on this backend"
answer, never a broken button.

# Design

## What it would take

Either:

- an agent-side summary call: ask the deployment's configured ElevenLabs
  agent for the Discuss summary, on the same fenced-input terms as message
  summaries, with the agent's own model (a property of the agent, not of this
  server's configuration); or
- a direct-LLM fallback: let a deployment configure `[summaries.llm]` for
  Discuss alone while message summaries stay on the agent backend. This splits
  the one-summarizer choice `summarizer_for` makes today and needs its own
  cache-key and refusal design.

## Current behaviour required until then

When no `[summaries.llm]` exists, Discuss is hidden or disabled with a clear
message saying it needs the direct-LLM summarizer configuration. The
mu-catchup lane implements that disabled state; this issue tracks the
agent-backend path above so the disabled state is a decision, not an omission.

# Acceptance Criteria

- A deployment on the agent backend either gets working Discuss through one
  of the two paths, or keeps the clear disabled state with a test pinning it.
- No wording describes the agent backend as legacy or deprecated.
