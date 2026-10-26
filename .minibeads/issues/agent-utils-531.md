---
title: 'server-side-syntax-highlight: highlight fenced code blocks on the server, with class spans the stylesheet colours'
status: open
priority: 1
issue_type: feature
labels:
- vibe-talk
created_at: 2026-10-26T14:33:49.925519289+00:00
updated_at: 2026-10-26T14:33:49.925519289+00:00
---

# Description

[opus 5.5] Owner, 2026-10-26: code blocks in rendered message bodies have no highlighting, because comrak is built without its syntect feature. Wanted: GOOD server-side Markdown rendering.

- Fenced blocks that name a known language are highlighted on the server with syntect through comrak's highlighter adapter, as spans carrying a short fixed list of hl-* classes; the stylesheet colours them for light and dark.
- Languages: Rust, Python, TypeScript/TSX, JavaScript, bash, JSON, YAML, TOML, diff, C, C++, Go, SQL, Markdown, Haskell, OCaml, Lean 4, Agda, Rocq (Coq). Agda and Rocq from MIT-licensed upstream TextMate grammars, converted to sublime-syntax.
- Unknown or missing language: plain monospace, never guessed.
- Sanitizer allows exactly the hl-* classes on span.
- Long lines scroll sideways inside the block.
- Rendered bodies are cached, so the timeline does not pay for highlighting on every refresh.
- Tables: comrak's table extension was already on; tables failed because of missing newlines (markdown-tables lane), not comrak.
