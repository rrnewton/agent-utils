---
title: 'muse-ember-summarizer: try Muse Ember for vibe-talk and wrkviz summaries'
status: closed
priority: 1
issue_type: feature
created_at: 2026-11-10T20:40:00.000000000+00:00
updated_at: 2026-11-10T20:40:00.000000000+00:00
---

# Description

The owner (2026-11-10) asked to try Muse Ember, the new small efficient Muse model positioned against small fast models from other vendors, for the two summarization surfaces that today run Muse Spark 1.3, and to switch if it is at least as good and cheaper or faster. A follow-up the same day added the newest Spark as a third arm: if Muse Ember does not win, the default becomes that newest Spark rather than staying on 1.3.

What runs today (verified 2026-11-10, not assumed):

- wrkviz (`py/wrkviz/muse_backend.py`): phase summaries, agent names and the project overview default to `muse-spark-1.3-internal` through the headless muse CLI; calendar rollups use a larger preview model on the same path.
- vibe-talk (live Slack instance overlay, read-only): message summaries and thread names use `muse-spark-1.3-internal` through the AI gateway chat-completions backend added in #1027 vibe-talk-spark-summarizer, at low reasoning effort. The owner remembered this as Spark 1.4; the running configuration is 1.3 on both surfaces.

# Design

- Three arms on real inputs, about 20-30 items per kind: the current Spark 1.3, the newest Spark served on each path, and Muse Ember under the public model name the Muse Models FAQ publishes for Muse Code and the Meta Model API.
- wrkviz kinds: phrase, paragraph and daily summaries for one team-day of this harness, through the muse CLI path wrkviz uses. vibe-talk kinds: message, thread-name and catch-up summaries over stored conversation data, read-only, through the AI gateway path vibe-talk uses.
- Measure blind-labelled quality side by side (a few short examples in the report), latency p50/p95, tokens and dollars per item and per day at current volume. Total experiment spend stays small; going over $20 needs coordinator approval first.
- The switch, where a challenger wins, is configuration only, in the spirit of #1027: wrkviz changes its named default model constants; vibe-talk needs no code change if the winner is served by its existing gateway backend, and the coordinator is sent the exact overlay line plus the landed SHA to apply and deploy. Every switch keeps a one-key rollback: one config value back to the previous model.
- If Muse Ember is not yet served to this host's identities on either path, that is reported as the blocker it is, with the exact error, and the Spark-versus-Spark result decides the default. There is no silent fallback to another transport.

# Acceptance Criteria

- A short report in the harness repository records, per surface and kind: what runs today, each challenger's model id as actually called, quality verdict with examples, latency p50/p95, tokens and dollars per item and per day, and the decision per surface.
- Where a challenger wins, the new default is landed behind config with the one-key rollback named in the report; where it loses or cannot be called, the current or newest-Spark default stands and the report says why.
- Focused tests cover any wrkviz default change; the full validation gate passes on the exact landed tree.
- No live configuration under the operator's config directory is edited and nothing is deployed by this change.

# Notes

Related: #1027 vibe-talk-spark-summarizer (gateway backend, configuration-only switching), #839 wrkviz-muse-backend and #842 (wrkviz muse defaults and rollup split).

## Resolution

[watermelon-full-preview] Done. Muse Ember (`muse-ember-1.0` in the Muse Models FAQ, $0.10/$0.004-cached/$0.50 per million tokens against Spark's $1.25/$4.25 list) is not yet callable internally: the AI gateway model list has no Ember id, gateway chat returns `model_not_found` (92/92), and `muse exec` reports the model does not exist; the announcement says internal availability is coming in the coming weeks. No intake was filed (coordinator is asking the owner).

On real inputs with shipped prompts (vibe-talk 24 messages, 20 threads, 12 catch-up windows over the gateway; wrkviz 24 phases and 6 daily rollups of one harness team-day through `muse exec`), Spark 1.4 (`muse-spark-1.4-internal`, already served on both paths) matched or beat the running Spark 1.3 on every kind: message quality 15-3, catch-up about 9 of 12, wrkviz phases about 20 of 24, thread names tied with valid slugs 20/20 on both, thread-name latency p95 11 s against 26 s at an equal 5.9 s message p50. Daily rollups keep `watermelon-full-preview` (best at n=6; both Sparks reported launched work as completed somewhere). Full numbers and examples are in the harness A/B report.

Landed switch: wrkviz `DEFAULT_MUSE_MODEL` is `muse-spark-1.4-internal` in Python and Rust (help strings included), rollback `--model muse-spark-1.3-internal` or that string in the constant. vibe-talk is configuration-only as designed in #1027: the coordinator applies `model = "muse-spark-1.4-internal"` in the live overlay `[summaries.llm]` and deploys; rollback is the same line with `muse-spark-1.3-internal`. No live config was edited here. Validation: focused wrkviz tests pass; the fixed Rust-help differential passes 5/5 solo; the full gate's only reds were the quarantined wrkslots class under host OOM (untouched, skipped per coordinator direction) plus the issue-number collision this renumber resolves.
