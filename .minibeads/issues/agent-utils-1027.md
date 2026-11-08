---
title: 'vibe-talk-spark-summarizer: summarise with Muse Spark 1.3 through the AI gateway'
status: closed
priority: 1
issue_type: feature
created_at: 2026-11-07T21:41:55.000000000+00:00
updated_at: 2026-11-08T01:37:11.000000000+00:00
closed_at: 2026-11-07T22:22:34.000000000+00:00
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

[watermelon-full-preview] Implemented by mu-spark. There was no blocker: the gateway Muse itself uses serves `muse-spark-1.3-internal` directly from this host, including to the client identity vibe-talk already runs with (mutual TLS, no key and no bearer token; with no client certificate the handshake is refused, as it should be). Its wire shape is chat completions, not Messages, so this landed as designed: a `meta_gateway` backend in `vibe-talk/src/summarize/meta_gateway.rs` behind the `Summarizer` trait, for summaries and thread names alike, sharing the Messages backend's client, identity reload, rate limiter, retry policy and cache. No `muse exec` fallback was needed or built. Existing sections keep their behaviour and their cache keys byte for byte (a golden test pins the pre-existing policy version).

Measured 2026-11-07 on this host, sequentially, with vibe-talk's verbatim prompts and fence, over 33 synthetic real-shaped inputs (27 long messages, 247–2,391 characters, plus 6 thread excerpts; no live chat content). Spark went through the gateway at `max_tokens = 1024`; the Haiku column is the current live path at its configured `max_tokens = 120`. Every call succeeded on both sides (33/33, all clean stops):

| Path | p50 | p95 | Mean |
|---|---|---|---|
| Spark 1.3, `reasoning_effort = low` (the shipped default) | 4.04 s | 5.59 s | 4.24 s |
| Spark 1.3, `reasoning_effort = minimal` | 1.78 s | 2.71 s | 1.93 s |
| Haiku 4.5, current path | 0.83 s | 1.06 s | 0.87 s |

By kind at the default effort: messages p50 3.83 s / p95 5.28 s against Haiku's 0.84 s / 1.28 s; thread names p50 5.46 s / p95 6.62 s against 0.82 s / 0.87 s. Spark is roughly five times slower than Haiku and roughly seven times faster than the bake-off's `muse exec` figure, comfortably inside the 30-second request timeout; thread names are background work regardless. At low effort the model averaged about 600 reasoning tokens per answer and peaked at 954 of the 1,024 ceiling, which is why the shipped `max_tokens` default for this backend is 2,048: the budget is a ceiling the model stops short of, and longer real messages with context need the headroom. A budget spent entirely on reasoning returns no text and is a named shape error, never an empty summary.

Quality read (mu-spark judging): Spark's message answers fit the brief — one sentence under the 160-character target every time (median 121 characters, 0 of 27 over) where Haiku's median was 221 characters with 25 of 27 over target, still one sentence but a long one to hear read aloud. Haiku kept more figures (see the experiment example: its extra clause is worth having and Spark's terser line drops it), and nothing on either side invented a fact on the message inputs; the injection-shaped input was reported, not obeyed, by both. On thread names every Spark slug was valid (12 of 12 across both efforts) where Haiku produced one five-word slug the name parser must reject, and one sentence with the rollout days transposed. Five examples, inputs shortened to their opening:

1. Incident note (build queue stuck since 02:40, lone runner hung). Spark: "The overnight builds are stuck because one machine stopped responding, delaying nightlies but yesterday's release still works." Haiku: "An arm64 build runner stopped responding overnight, blocking the build queue since 02:40, and the team is capturing a thread dump before restarting it to assess losing six hours of cached build artifacts." Both right; Spark's is the one a driver can absorb.
2. Deploy log dump ending in rollback. Spark: "A production deployment failed on the export feature and was automatically rolled back after briefly affecting users." Haiku opens "Build 4821 deployment to host two failed after triggering 500 errors…", quoting identifiers the prompt expressly forbids quoting.
3. Experiment readout (setup completion 54% to 71%). Spark: "New accounts using the shorter setup finished more often, so the team plans to roll it out to everyone and make invites skippable." Haiku keeps the percentages Spark drops, at about twice the length — the clearest point in Haiku's favour on this sample.
4. Thread: flaky checkout test. Spark: `checkout-banner-flake` / "swapping flat two-second wait for network-response wait after load pushed timing to 2.2s, fifty loaded runs green". Haiku: `checkout-flow-flaky-banner-wait` — five words, so the naming would fail validation outright — with a sound sentence behind it.
5. Thread: staged rollout (internal Monday, ten percent Wednesday 10:00). Spark: `sync-rollout-plan` / "starting with internal accounts Monday, then ten percent Wednesday at 10:00 with flag rollback and error-rate graphs at each step." Haiku: `sync-rollout-schedule` / "Wednesday morning launch for internal accounts, then ten percent…" — the Wednesday step attached to the wrong cohort.

Unverified here: gateway behaviour under sustained production rate (the client-side limit and 429 handling are the shipped ones, tested against a local mock), and long-run certificate rotation in service (the reload is the Messages backend's, likewise mock-tested; the live switch is the coordinator's deploy, not this change's). No live configuration was edited and nothing was restarted.

Addendum (owner follow-up, same day): switching stays configuration-only, and a second vendor's model was probed to prove it. The owner approved Spark at low effort and asked that it stay swappable — back to a Haiku-class Messages model or on to `gpt-6-luna` if available. It is available, with one wire difference: Luna is not on the Spark gateway (its model list holds only that gateway family's models; the id answers model-not-found), but its own per-model chat-completions gateway answers this server's existing client identity directly — tiny prompt and five of the sample inputs all succeeded at the shipped prompts, p50 1.20 s at low effort. Its deployment refuses `max_tokens` and requires `max_completion_tokens`; the Spark gateway accepts both spellings and honours the new one identically, so the client now sends the one spelling both take, and `reasoning_effort = "off"` omits the effort field for any future model that refuses it. No per-model branch was added: `model` is sent exactly as written, and the Haiku switch-back remains the `messages_api` backend. The README's Summaries section carries all three complete overlay blocks.
