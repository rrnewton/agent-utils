---
title: 'discuss-warmup-context: a bounded, fingerprinted warm-up for Discuss'
status: closed
priority: 0
issue_type: feature
assignee: mu-discussq
labels:
- vibe-talk
created_at: 2026-11-12T13:40:00+00:00
updated_at: 2026-11-12T13:40:00+00:00
---

# Description

[watermelon-full-preview] Owner reply 413: "Maybe we need to wire it to read some project READMEs (plus last weeks weekly summary?) or something as part of its warm-up to make it ready to operate. We could get a quantitative sense of how many terms it doesn't understand... The Discuss agent can become a translator that understands all the agent slop language in flight and can explain as needed to the user."

So the catch-up model starts cold on project jargon every run. This issue gives it a bounded warm-up — operator-named reference files (project READMEs, a glossary, the last weekly summary) prepended to the request as a stable, cache-friendly prefix — and measures what that buys against the #1351 discuss-unknown-terms-metric count. Owner reply 414 applies throughout: the warm-up is local files only, and Discuss time stays model time only. This issue owns the warm-up assembly in `vibe-talk/src/catchup.rs` and its `[summaries]` config; the window context/IO path stays mu-catchup's.

# Design

- **Config.** `[summaries] catchup_warmup_files` (list of paths, in order; default empty = no warm-up, exactly as before) and `catchup_warmup_max_chars` (default 6,000 — about 1,500 tokens). Both are classified as changing no word of a summary, so neither enters the summary cache key (the summarize destructure guard pins this).
- **Assembly.** `assemble_warmup` renders a standing "Reference material … (background only)" header and one `## Reference: <file name>` section per file, cut once at the whole budget with the cut marked. It is the first thing in the request after the system prompt, so the prefix is stable across runs where the gateway caches prefixes.
- **Caching and fingerprint.** `load_warmup` fingerprints the files (path + mtime + length, hashed) and re-reads only when the fingerprint moves; the endpoint logs `warmup_chars` and `warmup_fingerprint` on every answered run beside the unknown-terms count, so the live trend joins the two. An unreadable file is left out with a warning, never an error.
- **Starter glossary.** `vibe-talk/prompts/discuss-glossary.md` ships the terms the measurement below kept surfacing (lane names, coordinators, Herdr, agentctl, the model names); an operator points `catchup_warmup_files` at it, plus their READMEs and weekly summary.

# Acceptance Criteria

- With no files configured, a Discuss read is byte-for-byte the request it was before; with files, the warm-up heads the request within its budget and the log line names its size and fingerprint.
- A changed file is picked up without a restart (fingerprint moves), an unreadable one degrades to a warning, and unit tests pin the assembly, the cut, and the fingerprint.
- The token/latency cost is measured against the #1351 unknown-terms count on the same windows and recorded in Notes, with a proposed size.
- The change is gated at owner tier and landed on its own, after #1351, with this issue file.

# Notes

[watermelon-full-preview] #1352 discuss-warmup-context, mu-discussq. Measurement (2026-11-12, live gateway wire, same 3 cached windows of 30 messages as the #1350 comparison, small-elk-internal, medium effort, single runs): a 4,056-char warm-up (the shipped glossary plus a 2,500-char README head) added 923–2,480 prompt tokens and about +0.4–0.9 s per run (4.0–4.2 s vs 3.1–3.8 s baseline). Unknown-terms counts did NOT fall: 8/11/5 (mean 8.0) against the 4/9/7 baseline (mean 6.7). The glossary's own terms (mu-\*, coordb, Herdr, agentctl, Spark, MCP) largely vanished from the lists, but the model surfaced other marginal terms instead, and single runs are noisy. Proposed size: keep the 6,000-char budget, but spend it glossary-first — a maintained glossary of the recurring terms, not README prose, which cost the most tokens for no visible gain. The honest verdict is that n=3 single runs cannot settle this; the mechanism ships with the metric attached (`unknown_term_count` beside `warmup_fingerprint` per run), so the live stamp trend over real use is the measurement that decides whether the warm-up earns its tokens.
