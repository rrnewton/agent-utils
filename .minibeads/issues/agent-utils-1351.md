---
title: 'discuss-unknown-terms-metric: count the terms the catch-up cannot explain'
status: closed
priority: 0
issue_type: feature
assignee: mu-discussq
labels:
- vibe-talk
created_at: 2026-11-12T11:20:00+00:00
updated_at: 2026-11-12T11:20:00+00:00
---

# Description

[watermelon-full-preview] Owner reply 413: "The summary agent should be encouraged to list any terms it thinks it does NOT understand in a debugging section for our own further development... Project tracking jargon (step 3A, wave Q, blah blah blah) is a very common source of indecipherable agent text. If we're doing things right these should show up as unknown/undefined in our diagnostic section and we can quantitatively reduce that over time by a better approach. The Discuss agent can become a translator that understands all the agent slop language in flight and can explain as needed to the user."

This issue makes that a number: every Discuss answer ends with a "Terms I didn't understand" section, the server parses and counts it on every run, and the count plus the terms are logged and stamped so the trend can be driven down (by #1352 discuss-warmup-context and glossary work). Owner reply 414 applies throughout: cache-only context, model time only. This issue owns the prompt instruction and the server parsing/counting in `vibe-talk/src/catchup.rs`; the panel's collapsed drawing is mu-catchup's.

# Design

- **Prompt.** After the workstreams, the answer ends with a section headed exactly `## Terms I didn't understand`: one `- <term>` per term, acronym, codename, or project-tracking phrase whose meaning the model could not work out from the messages, or `- None`. The list is a debugging aid outside the 1,200-character briefing budget.
- **Parsing.** `split_unknown_terms` runs on the raw answer before shaping: it takes the trailing section out of the printed text (so the briefing stays clean and no citation inside it is linked or counted as cited), reads its bullets in order, dedupes them, strips backticks, treats `- None` and an empty section as none, and caps the list at 40.
- **Counting.** `CatchupResponse.unknown_terms` carries the terms for the panel's collapsed section; `CatchupStamp.unknown_term_count` carries the count; the endpoint's structured log line gains `unknown_term_count` and `unknown_terms` (comma-joined) on every answered run, beside the existing stamps.
- **Compatibility.** Both fields are absent when empty, and nothing else in the response changes; a panel that does not draw them yet is unaffected.

# Acceptance Criteria

- An answer with the section yields the parsed terms in the response, the matching count in the stamp, and both on the log line; an answer without it (or with `- None`) yields none and prints its full text.
- The section never appears in the shaped briefing text, and citations inside it are neither linked nor counted in `cited_ids`.
- Unit tests pin the parsing (dedupe, cap, `- None`, the nothing-new line), the contract samples cover the new fields, and the catch-up endpoint tests pass.
- The change is gated at owner tier and landed on its own, after #1350 discuss-summary-structure, with this issue file.

# Notes

[watermelon-full-preview] #1351 discuss-unknown-terms-metric, mu-discussq. The count is the baseline #1352 discuss-warmup-context is measured against: warm-up context (READMEs, a glossary, the last weekly summary) is worth its tokens and latency only if this number falls. Terms are model-reported, so the metric measures what the model admits it cannot explain, not an independent comprehension test; the same window set is reused across measurements so the numbers stay comparable.
