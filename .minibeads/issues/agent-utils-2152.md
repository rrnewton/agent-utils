---
title: 'vibe-talk-summarizer-backend-interface: one interchangeable summarizer backend interface for direct API, ElevenLabs agent and warm Claude Code harness modes'
status: open
priority: 2
issue_type: feature
labels:
- vibe-talk
created_at: 2026-11-23T06:25:00-05:00
updated_at: 2026-11-23T06:25:00-05:00
---

# Description

Owner request 330 (2026-11-23): an outside team uses the ElevenLabs agent in text-to-text mode for summaries and wants that to extend to thread names, but has never had thread summaries or thread names working. The same report notes an unfinished option to use a warm Claude Code full harness instead of direct API summarization. The owner wants both modes supported behind one clean summarizer interface, with direct API mode remaining the default: direct API mode is expected to have better latency, while a full harness mode would allow subscription pricing. The backends must be interchangeable, and the harness mode must not become the default.

Today the backends are not interchangeable. Selection happens once in summarizer_for: a present summaries.llm section builds one of the direct backends named by its backend key, and an absent section builds the ElevenLabs agent backend. Only the direct backends name threads and answer the Discuss side agent. The ElevenLabs agent backend summarizes messages but silently never names threads and cannot answer Discuss at all. The warm Claude Code session backend exists, but it forces tools off, cannot run the Discuss tool loop, and cannot be deployed in the container launcher. An outside deployment that already runs an ElevenLabs agent for voice therefore gets message summaries with no thread names and no Discuss, and has no documented path to the other modes.

Related reports: the ElevenLabs thread naming request, the container deployability request for the warm session backend, and the container overlay forwarding request. The design document for this issue is ai_docs/plans/2026-11-23-summarizer-backends.md in the harness repository.

# Design

One backend interface covers four operations for every backend: message summary, thread name plus sentence, Discuss first answer, and Discuss follow-up with tools. A backend declares its capabilities explicitly (names threads, answers Discuss, runs tool follow-ups) and the server reports any missing capability by name at startup and per request, never by silent omission. The existing Summarizer trait is the starting point: summarize, name_thread, complete_prompt and complete_followup keep their signatures, and defaults that return NotConfigured remain, but the thread path must surface a named unavailable reason when naming is not offered.

Three backends implement the interface. First, the direct API backends (Messages API and AI gateway chat completions) remain the default: one HTTP call per operation, per-kind models with fallback chains, the shared rate limiter, and the existing cache policy versioning. Second, the ElevenLabs agent backend gains thread naming over the same pooled text socket it already uses for summaries, with the same plain-text discipline and answer parsing as the direct backends, and gains a Discuss first answer over that socket; tool follow-ups on this backend either run through a server-side tool loop whose results are fed back as text, or refuse by name if measurement shows the agent cannot sustain the loop. Third, a warm Claude Code harness backend (opt-in only, never the default) keeps a small pool of warm sessions with tools exposed through a local MCP server backed by the same cache-only tool code the gateway loop uses, so the harness runs the full Discuss loop while every tool read still answers only from the server cache.

The cache-only guarantee holds for every backend: the six read-only Discuss tools (find_message, read_page, read_message, digest_channel, fetch_document, search) execute in-process against the cache, write-shaped calls are refused by name before any executor runs, and no backend may call the chat upstream to serve a tool. Untrusted channel text stays inside the existing fence construction, and prompt or backend changes keep invalidating cached summaries through the policy version.

Configuration keeps one home: the summaries.llm section names the backend and its settings, with backend-specific keys refused by name on the wrong backend, and no secret ever inline. The container launcher learns to deliver the section and, for the harness backend, a scoped credential and the harness program, so an outside team can select any backend by configuration alone. Migration is additive: existing configurations keep their current backend, cache keys and behavior byte for byte, and each new capability lands behind the existing trait so call sites do not change.

# Acceptance Criteria

- Thread naming works on the ElevenLabs agent backend through its pooled text socket, proven against the offline mock, and a deployment on that backend no longer silently omits names: either names appear or the startup banner and diagnostics name the gap.
- Discuss first answers work on every backend that declares the capability, and Discuss tool follow-ups work on the direct gateway backend as today and on the warm harness backend through the cache-only MCP tool server; a backend that cannot run follow-ups refuses by name.
- The warm Claude Code harness backend is selectable by configuration alone (including in the container launcher with a scoped credential recipe), is never the default, keeps its sessions warm with documented recycle and restart behavior, and exposes no tool beyond the six read-only cache-only tools.
- Direct API mode remains the default wherever summaries.llm is present, with unchanged cache keys for existing configurations, and switching backends is a configuration change only.
- Interface tests run against a fake backend plus each real backend against local mocks, covering summary, thread naming, Discuss first answer, tool follow-up allowlist refusal, failure mapping, and the cache-only assertion that no tool call reaches the chat upstream.

# Notes

Filed by mu-sumbackend at the request of the lead coordinator for owner request 330. Design detail, per-backend latency and cost expectations, failure and fallback behavior, config shapes, install requirements for outside teams, the migration path and the test plan live in the design document named in the Description. No live service was contacted and no credential was read while filing this issue.
