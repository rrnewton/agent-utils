---
title: 'discuss-answer-whitespace: rendered bullets draw without blank lines'
status: closed
priority: 0
issue_type: bug
assignee: mu-discussws
labels:
- vibe-talk
created_at: 2026-11-12T19:24:00+00:00
updated_at: 2026-11-12T19:24:00+00:00
---

# Description

[watermelon-full-preview] Owner Slack 010 (via coordb), on the #1349 discuss-markdown-render deploy: "I see bullets now but EXCESSIVE whitespace between and before/after bullets." Cause: `vibe-talk/web/styles/voice/22-discuss.css` kept `#discuss-answer { white-space: pre-wrap; }` from the plain-text era. Now the panel draws the server's sanitized `text_html` (view/catchup/panel.ts, through view/dom/html.ts), so the newlines between `<ul>`/`<li>`/`<p>` tags render as blank lines, on top of the block margins.

Parent: #1316 catchup-side-agent. Builds on #1349 discuss-markdown-render (landed).

# Design

- `pre-wrap` applies only to the plain-text fallback: panel.ts toggles a `discuss-answer--plain` class on `#discuss-answer` (set on the text fallback, cleared on the HTML render); the rendered HTML gets normal white-space.
- List spacing tightened: `ul`/`ol` margin 2px 0 with `padding-left: 1.2em`, `li` margin 0, and `p` inside `li` margin 0. Headings stay compact; the first/last child of the answer carries no outer margin.

# Acceptance Criteria

- A rendered bullet list has no blank lines between items and no excess space before/after the list, at phone and desktop widths.
- An answer without `text_html` still draws as plain text with its newlines kept, and model text never reaches innerHTML except through `text_html`.
- The catchup page test asserts the class toggle both ways; the focused suite passes.

# Notes

Owner Slack 010 via coordb. Number reserved by coordb. Batched with #1392, #1347, #1384, #1382+#1383 under owner reply 012 (integrator: mu-discussfoot).

## Resolution

Landed on main as e5fc567c in the owner-reply-012 batch (integrator mu-discussfoot; gated 851343ad, replayed with --allow-overlap) and deployed live about 5:53 PM ET. The plain-text fallback keeps `pre-wrap` via `discuss-answer--plain`; rendered HTML draws with normal white-space and tight list spacing. Focused catchup test passed before the batch gate.
