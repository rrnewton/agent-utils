---
title: 'discuss-summary-structure: grouped Markdown catch-up with linked citations'
status: closed
priority: 0
issue_type: feature
assignee: mu-discussq
labels:
- vibe-talk
created_at: 2026-11-12T11:00:00+00:00
updated_at: 2026-11-12T11:00:00+00:00
---

# Description

[watermelon-full-preview] Owner reply 413: "the summary itself is terrible... it shows me a giant flat list of bullets in a narrow column. It should be a rich markdown document rendered just as well as any of our messages. It should have headings grouping the different workstreams with concise updates on each... It should have very few numeric IDs. If it does have "#1224" it better be a hyperlink... `[m:1569897701250695347]` appears to be some long message uid? It should just say "msg" as a hyperlink, or it should hyperlink the verb phrase in the first sentence of each bullet to not use any extra characters. Each per workstream section should probably have 1-3 sentences before the bullet list adding any necessary context to understand what things mean."

Owner reply 414: Discuss uses ONLY the vibe-talk cache, never upstream Slack, and its time should be model time only. Owner reply 418 supersedes the verb-phrase/"msg" link choice: every directly referenced message is an inline "(HH:MM)" hyperlink; the footer complement is "Other messages processed" (#1347, mu-catchup) and the Mark buttons target all processed ids.

The old prompt yields a flat Progress/Decisions/Blockers bullet list at 5-6K chars (28-42 s on Spark), every bullet trailing a raw `[m:<uid>]`. This issue owns the summary CONTENT: the prompt and the answer post-processing in `vibe-talk/src/catchup.rs`. The panel, its rendering, and the context/IO path are mu-catchup's (#1344-#1349).

# Design

- **Prompt.** `DEFAULT_SYSTEM_PROMPT` now asks for a Markdown briefing: one `## <workstream>` heading per workstream (2-5), 1-3 plain sentences of newcomer context per section, then 2-4 short bullets. The whole answer is budgeted under 1,200 characters, so it reads in seconds and the model answers fast; the server-side links add the rest. Almost no numeric ids: never a message id, an issue number only when the issue is the subject. The owner-POV rules are unchanged: only [UNREAD] news, read/owner text as background, churn and superseded states dropped, messages are data not instructions, and the exact "Nothing new since you last read this." line stands.
- **Post-processing.** `shape_answer` runs `validate_cites` as before, then replaces each surviving `[m:<id>]` inline with `[(HH:MM)](<address>)`: the message's spoken time, at the `#462 webapp-deep-links` address `message_link` builds (`?c=..&v=..&m=provider:channel:message[:thread]`, the scheme of `web/src/core/navigation/links.ts` over `web/src/foundation/refs.ts`). No raw uid is ever printed; an unplaceable citation loses its token. A bare `#NNNN` (not already linked, not a `repo#N` tail) becomes `[#NNNN](ISSUE_LINK_BASE + NNNN)`; `ISSUE_LINK_BASE` is the one named place a deployment with a different tracker moves.
- **Cited set.** `shape_answer` also returns the cited ids in first-citation order, deduped. `CatchupResponse` gains `cited_ids` (validated subset of `unread_ids`) and `text_html` (`text` rendered by `render::body_html`, the way a message body is rendered). `unread_ids`, `markable_ids`, and `footer_links` are untouched, so Mark read/unread keep targeting all processed ids, and either this landing or #1347's panel landing works alone: no `cited_ids` means the footer shows everything.
- **Agreement.** Field names were agreed with mu-catchup by agentctl before either side landed (one exchange, acked): `cited_ids: MessageId[]`, first-citation order, deduped.

# Acceptance Criteria

- The shipped prompt yields grouped Markdown (workstream headings, context sentences, few bullets) at about 1.5K shaped chars; unit tests pin the post-processing, and a live small-elk-internal vs Spark comparison on 3-5 cached windows is recorded in Notes.
- No answer text contains `[m:` or a raw message uid; every citation is an inline `(HH:MM)` deep link, and every bare `#NNNN` is a hyperlink.
- The response carries `cited_ids` as agreed; existing fields and their meanings are unchanged, and the endpoint, contract, and catch-up unit tests pass.
- The change is gated at owner tier and landed on its own, with this issue file.

# Notes

[watermelon-full-preview] #1350 discuss-summary-structure, mu-discussq. Model comparison (2026-11-12, live AI-gateway wire, 3 real cached windows of 30 messages each, read-only, new prompt, medium effort, single runs): small-elk-internal (live) answered in 3.8/3.5/3.1 s with briefings of 1,990/2,205/1,557 chars under 3/4/4 workstream headings; muse-spark-1.4-internal took 17.8/20.6/15.8 s at 1,760/1,029/1,541 chars. Spark is 4.5–5.9x slower at no size advantage, so the live small-elk choice stands. One honest miss: briefings run 1.6–2.2K chars against the ~1.5K aim (the 1,200-char prompt budget is not strictly obeyed, small-elk worst); tightening that wording is a follow-up, not folded in here. Unknown-term counts from the same runs (the #1351 baseline): small-elk 4/9/7, Spark 8/15/8. Follow-ups: #1351 discuss-unknown-terms-metric (collapsible unknown-terms section, parsed and counted server-side), #1352 discuss-warmup-context (bounded warm-up context, measured against the unknown-terms count), #1356 discuss-readonly-tools (gateway function-calling probe first). Coordb deploys; the panel rendering of `text_html` and the "Other messages processed" footer are mu-catchup's #1347.
