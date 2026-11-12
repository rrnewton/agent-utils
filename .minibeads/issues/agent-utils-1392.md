---
title: 'discuss-citation-links: Discuss citations render as in-app anchors, not plain text'
status: closed
priority: 0
issue_type: bug
assignee: mu-discusscite
labels:
- vibe-talk
created_at: 2026-11-12T21:08:48+00:00
updated_at: 2026-11-12T21:08:48+00:00
closed_at: 2026-11-12T21:08:48+00:00
---

# Description

[watermelon-full-preview] Owner report (via coordb) on the #1349 deploy: the Discuss answer's bullets show citations as plain text, e.g. `(08:19) (?c=...&v=all&m=...)`, so #1349's "citation taps jump in-app" never gets an anchor. The model is not at fault: `catchup::shape_answer` already turns each citation into a Markdown link `[(HH:MM)](?c=...&m=...)`, but the answer is then rendered with the channel-message renderer (`render::body_html`), whose ammonia sanitizer denies relative addresses and unwraps every refused link to `text (url)`.

Parent: #1316 catchup-side-agent.

# Design

- A Discuss-specific render entry point (`render::body_html_catchup`) renders the shaped answer with a trusted set: exactly the `?c=...&m=...` addresses the server itself built for that answer, recomputed in `http::api::catchup` through the same `cite` closure `shape_answer` used.
- A link keeps its `href` only on exact membership in that set, checked twice: in `render::adjust` (a non-member relative link is unwrapped as today) and in a per-answer ammonia copy whose `UrlRelative::Custom` evaluator admits only set members. Every other sanitizer rule is shared with the channel sanitizer, which keeps `url_relative(Deny)` and is otherwise untouched.
- No page change: the panel's existing `answerTap` already recognises an anchor whose `href` carries `?` and an `m=` ref, and jumps in-app.

# Acceptance Criteria

- A cited answer rendered through the real api render path contains an `<a>` with the in-app href and no `(?c=` plain-text citation.
- A model-written relative link in the raw answer (same shape, not in the trusted set) is still refused, as is every relative link on the channel path.
- A tap on the rendered anchor jumps in-app (already pinned by the page suite's citation-tap test).

# Notes

Owner fast-track via coordb; ships in the owner reply 012 small-fix batch. Number reserved by coordb.

## Resolution

[watermelon-full-preview] Fixed on branch `mu-discusscite/1392` for the batch integrator:

- `vibe-talk/src/render.rs`: `body_html_catchup(text, markup, trusted)` plus the trusted set threaded through `render_visiting`/`adjust`/`write_html`; the static channel `SANITIZER` is rebuilt from a shared `sanitizer_builder()` and still denies relative addresses. `TrustedRelative` is the ammonia evaluator admitting only exact set members.
- `vibe-talk/src/http/api.rs`: the Discuss handler names its `cite` closure, shapes with it, then collects the trusted set from the cited ids through the same closure and renders with `body_html_catchup`.
- Tests (synthetic ids only): `catchup::tests::a_cited_answer_renders_its_citations_as_in_app_anchors` runs a cited answer through the real api render path and asserts the anchor with no `(?c=` text; `render::tests::a_discuss_answer_keeps_only_its_trusted_citation_links` asserts a model-written relative link and the channel path still refuse. Focused evidence: both new tests pass; full `cargo test --lib` 1868 passed; `cargo clippy --lib` clean; page suite citation-tap test pre-exists in `tests/js/view/catchup.test.mjs`.
