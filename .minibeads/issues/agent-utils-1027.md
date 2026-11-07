---
title: 'vibe-talk-spark-summarizer: summarise with Muse Spark 1.3 through the AI gateway'
status: open
priority: 1
issue_type: feature
created_at: 2026-11-07T21:41:55.000000000+00:00
updated_at: 2026-11-07T21:41:55.000000000+00:00
---

# Description

The owner (2026-11-07), on vibe-talk's summaries running on `claude-haiku-4-5`: "I wanted that on Muse spark. What was the blocker there". There was no blocker; Muse Spark was only ever wired into wrkviz (#839 wrkviz-muse-backend, #842), which defaults to Muse Spark 1.3 (`muse-spark-1.3-internal`) through `muse exec`. vibe-talk's `src/summarize/` has two backends: the direct Messages API caller (`[summaries.llm]`, #233 llm-summarizer) and the ElevenLabs agent. Neither can ask for Spark.

`muse exec` is the wrong vehicle per message: the 2026-11-03 summarisation bake-off measured about 27 s and about 20k tokens of harness overhead per Spark call, for a one-sentence answer. vibe-talk summarises interactively (a reader waits on a spinner) and in bulk (thread names), so it needs the direct call the bake-off noted would avoid that overhead: the AI gateway (Meta Model API upstream) that Muse itself uses. In that bake-off Spark 1.3 placed ahead of every Haiku tried, and `claude-haiku-4-5` placed last of ten, with the only factual errors.

# Design

- A third summariser in `vibe-talk/src/summarize/`, selected by a new optional key under `[summaries.llm]`: `backend = "meta_gateway"` with `model = "muse-spark-1.3-internal"`. Absent (or `backend = "messages_api"`) keeps today's behaviour byte for byte, so every existing configuration, including overlays that replace `[summaries.llm]` whole, works unchanged.
- The gateway call is one plain HTTPS request in the gateway's OpenAI-compatible chat-completions shape: a system instruction and one user turn, no tools field the type could even express (the #233 rule), authenticated by the mTLS client identity vibe-talk already supports (`client_identity_file`, `ca_bundle_file`, re-read on rotation). `base_url` is required configuration with no built-in default, so no deployment address is baked into the binary. Credentials stay in files, never inline, never logged.
- Spark spends output budget on reasoning before it writes, so the backend carries a `reasoning_effort` setting (default low) in the request and its own `max_tokens` default with headroom for reasoning plus one sentence; an answer cut off by the budget with no text is a shape error, not an empty summary. Both values join dialect/model/`max_tokens` in the cache policy version, because they change what a summary says.
- Both callers move together: per-message summaries and thread names (`Summarizer::summarize` and `Summarizer::name_thread`) go through the same backend, the same shared rate limiter, the same retry-on-429/overload policy, the same timeouts, and the same summary cache. The fenced-untrusted-input construction (`SummaryRequest`, `ThreadRequest`) is untouched.
- If the gateway cannot serve Spark to the identity a deployment runs as, that is reported as the blocker it is; there is no silent fallback to `muse exec` or to another model.

# Acceptance Criteria

- `backend = "meta_gateway"` summarises a message and names a thread against a local test server speaking the gateway shape; request-shape tests assert no tools field, the configured model and reasoning effort, and mTLS identity handling, mirroring the Messages API tests. Misplaced keys (a Messages API dialect, project/region, API-key or bearer settings on this backend) are refused by name at load.
- Existing configurations parse and behave exactly as before (the current `[summaries.llm]` test suite passes unmodified in what it asserts), and summaries cached under the old policy version stay reachable until the backend or model actually changes.
- A measured comparison on 30 or more real-shaped inputs (long messages plus thread-name inputs, from test fixtures or synthesised; no live chat content in the repository): p50/p95 latency per call for Spark through the gateway against the current Haiku path, and a side-by-side quality read with five quoted examples, recorded under Resolution below.
- The exact push tree passes the full validation gate, `check_client_names` and `cargo fmt --check`. No live configuration is edited and nothing is restarted by this change; the overlay lines a deployment needs are reported to the coordinator instead.

# Notes

[watermelon-full-preview] Filed by mu-spark. wrkviz's Spark precedent is #839 wrkviz-muse-backend and #842; its transport (`muse exec` per batch) suits batch jobs and is deliberately not reused here.

## Resolution

Pending: gateway reachability, latency and quality numbers land here with the implementation.
