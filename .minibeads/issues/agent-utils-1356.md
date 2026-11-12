---
title: 'discuss-readonly-tools: read-only tool use for Discuss follow-ups'
status: open
priority: 1
issue_type: feature
assignee: mu-discussq
labels:
- vibe-talk
created_at: 2026-11-12T13:50:00+00:00
updated_at: 2026-11-12T13:50:00+00:00
---

# Description

[watermelon-full-preview] Owner reply 419: "By the way, if we have continued chat with the agent but it is NOT a full TUI/agentic harness... is it just text-in / text-out? Right now we are PUSHING to it a bunch of text and asking it to summarize. If we have subsequent discussion with it is it possible to hook it up to the vibe-talk MCP/CLI and give it limited tool use to pull more context? Or would that require a bunch of reinventing the wheel?"

Today the side agent is one text-in/text-out call. Follow-up turns (mu-catchup's #1353 composer) may call a READ-ONLY subset of the `vibe-talk/src/mcp/mod.rs` manifest tools — `find_message`, `read_page`, `read_message`, `digest_channel`, `fetch_document`, and `search` after #1338 — offered as chat-completions `tools` and executed in-process through `crate::ops`. Hard bounds: cache only (owner reply 414, never upstream), a bounded loop (about 4 steps), never `post_reply` or `mark_read`, and the first summary stays tool-free. No wheel is reinvented if the gateway passes function calling on the wire vibe-talk already speaks; step 1 probes exactly that.

# Design

- **Step 1 (probe, done 2026-11-12).** One `tools` request per model on the deployment's meta_gateway wire (chat completions, mTLS, the `find_message` shape): small-elk-internal and muse-spark-1.4-internal both answered HTTP 200 in ~1.2–1.4 s with `finish_reason: tool_calls` and a well-formed `find_message` call for the asked id. So the gateway path carries function calling for both models, and the fallback — the #1331 claude_session backend (mu-claudesum) over vibe-talk's `/mcp` — is not needed. Build pick: the in-process loop on the existing gateway summarizer.
- **Step 2 (build, after #1353 lands).** A tool-using completion beside `complete_prompt` on the gateway backend (and the Messages API backend if its wire carries the same shapes): the read-only manifest subset as `tools`, tool results executed through `crate::ops` against the cache, at most ~4 model steps, then the final text shaped by the same post-processing as the first summary. The transcript/composer storage is shared with mu-catchup's #1353; this issue owns the loop and the tool wiring, not the panel.

# Acceptance Criteria

- The probe result is recorded per model with the request shape and one sample (done: Notes).
- Follow-up turns can call only the named read-only tools, in-process, cache-only, within the step bound; a test proves a write-shaped call is refused and the loop stops at the bound.
- The first (one-shot) summary makes exactly one tool-free call, as today; follow-ups carry the transcript #1353 stores.
- The change is gated at owner tier and landed on its own, after #1353, with this issue file.

# Notes

[watermelon-full-preview] #1356 discuss-readonly-tools, mu-discussq. Probe (2026-11-12, live wire, request: `tools: [{type: function, function: {name: find_message, parameters: {message_id}}}]`, `max_completion_tokens: 2048`, `reasoning_effort: medium`, user asking to look up one real cached id): small-elk-internal — HTTP 200, 1.2 s, `finish_reason: tool_calls`, `tool_calls: [{name: find_message, arguments: {"message_id": "1570414242325070713"}}]`; muse-spark-1.4-internal — HTTP 200, 1.4 s, same shape. Both models pass function calling, so step 2 builds on the gateway wire; the #1331 claude_session fallback is the pick only if a future deployment's gateway refuses `tools`. Build ETA: about 3–4 h after #1353 discuss composer/transcript lands; coordinate the transcript seam with mu-catchup before building.
