---
title: 'vibe-talk-github-login-error-text: document failures name vibe-talk and the real helper cause'
status: open
priority: 0
issue_type: bug
labels:
- vibe-talk
- documents
- ui
created_at: 2026-11-20T16:48:22-05:00
updated_at: 2026-11-20T16:48:22-05:00
---

# Description

[watermelon-full-preview] Owner fast-track (owner 267-269, 2026-11-20): a host reimage removed the configured GitHub helper, vibe-talk's `[documents] github_token_command` still pointed at the missing path, and every private-repository document failed while the page showed only a cut-off "token command failed: N". The error named neither the layer that failed nor the real cause.

# Design

- The token-helper failure in the documents fetcher is worded at the vibe-talk layer: "vibe-talk could not get your GitHub login: the helper <cmd[0]> is missing", "... the helper exited <code>: <first stderr line, trimmed>", and "... the helper printed no token", for the missing, non-zero-exit, and empty-output cases respectively.
- Only the first trimmed stderr line is ever included; the token and the full stderr are never included, logged, or returned.
- The server error mapping carries that sentence unchanged to the reader and to the document row, so both surfaces show the same specific cause instead of a truncated generic prefix.

# Acceptance Criteria

- Unit tests cover each case: helper missing, helper exits non-zero with multi-line stderr (only the first line, trimmed, is shown, with its exit code), and helper prints no token.
- No test or error text contains a token value or more than the first stderr line.
- The reader and a failed document row both show the layer-named sentence for a helper failure, proven with fake helpers only, never a real helper or token.

# Notes

[watermelon-full-preview] Pre-assigned number #1997 vibe-talk-github-login-error-text. Follow-up #1998 vibe-talk-github-helper-health-fallback covers startup/heartbeat checks and fallback helpers. No host names, local user paths, or real account names in committed files; placeholders only.
