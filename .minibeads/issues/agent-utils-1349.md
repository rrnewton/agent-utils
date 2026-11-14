---
title: 'discuss-markdown-render: the Discuss answer draws as rendered Markdown'
status: closed
priority: 0
issue_type: bug
assignee: mu-catchup
labels:
- vibe-talk
created_at: 2026-11-12T16:47:30+00:00
updated_at: 2026-11-14T16:14:54.000000000+00:00
closed_at: 2026-11-14T16:14:54.000000000+00:00
---

# Description

[watermelon-full-preview] Owner fast-track from the live panel (relayed by coordb): the Discuss answer shows as raw text — `##` headings, `[#1348](<url>)` links with angle brackets, `[(08:19)](?c=...)` citation links. The server already returns the shaped text plus its sanitized render (`text_html`, #1350 discuss-summary-structure); the panel still draws `text` as text only.

Parent: #1316 catchup-side-agent.

# Design

- When `text_html` is present, the panel draws it as markup (it is the server's own sanitized render of the answer, the way message bodies are rendered); otherwise it draws `text` as text, as today. The angle-bracket link form renders as a link either way it arrives.
- Citation links (`?c=..&v=..&m=..`) tapped in the answer jump in-app through the existing jump path; external links keep the renderer's new-tab dress.
- `unknown_terms` draw as a collapsed block under the answer (server-parsed, #1351); absent/empty draws nothing.

# Acceptance Criteria

- Headings, lists, and links (including `[text](<url>)`) render formatted at phone (390x844) and desktop; no raw Markdown or angle brackets show.
- A citation tap jumps to its message; an external link opens as rendered; an answer without `text_html` still draws as plain text, and model text never reaches innerHTML except through `text_html`.
- Unknown terms appear collapsed with their count. Page tests + Discuss walks pass; gated at owner tier, landed on its own.

# Notes

Owner fast-track via coordb (reply 005 session relay). Number reserved by coordb. Server fields: #1350/#1351 (mu-discussq, landed).

## Resolution

[watermelon-full-preview] Landed in 97c51ef3ab2f5d252ded56289c08c0c668b2ca5d on main and live in release ba329b5a. The Discuss answer now draws the server's sanitized `text_html` as markup (headings, lists and links formatted, citation taps jumping in-app) and falls back to plain `text` when no render is present; unknown terms draw collapsed. Verified as an ancestor of the live release and by `text_html` in the shipped server render path.
