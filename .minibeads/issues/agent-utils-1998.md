---
title: 'vibe-talk-github-helper-health-fallback: check the GitHub helper at startup and on a heartbeat, with fallback helpers'
status: open
priority: 0
issue_type: feature
labels:
- vibe-talk
- documents
- ops
created_at: 2026-11-20T16:48:22-05:00
updated_at: 2026-11-20T16:48:22-05:00
---

# Description

[watermelon-full-preview] Owner fast-track (owner 269, 2026-11-20): "Maybe we can Auto fix that or refresh it on a heartbeat in the future." After a host reimage removed the configured GitHub helper, vibe-talk kept failing private-repository documents with no standing signal that its GitHub login helper was gone, and no attempt to use another installed helper.

# Design

- vibe-talk checks the token helper at startup and on a heartbeat: a cheap run whose token output is discarded at once and never logged, stored, or returned; only the outcome (ok / missing / failing, with the #1997 cause sentence) is kept.
- The outcome is reported in `/healthz` and in the ops check (`vibe-talk doctor`), with the cause.
- If the configured helper path does not exist, a short fixed list of known installs is tried in order: the user-local helper under the home directory's `.local/opt/gh/usr/bin/gh`, then the helper on `PATH`. A candidate that is not a real helper binary is skipped — a script that forces a different `--user` than the configured one is refused, and an explicit denylist in config is honoured. The first candidate that returns a token for the configured `--user` is used; the switch is logged once (path only) and `/healthz` surfaces "using fallback <path>".
- The fallback list is configurable (`github_token_helper_fallbacks`) and can be turned off with an empty list; the denylist is `github_token_helper_denylist`.

# Acceptance Criteria

- Tests use fake helper scripts in a temporary directory only (missing, exits 1, prints nothing, prints a token); never a real helper, never a real token.
- `/healthz` names the helper state and cause, and names the fallback in use when one is; the ops check names a missing configured helper.
- A denylisted candidate and a script forcing a different `--user` are skipped for the next candidate; with fallbacks turned off, a missing configured helper stays missing.
- The token a check or fallback probe obtains is never logged, stored in the health state, or returned by any endpoint.

# Notes

[watermelon-full-preview] Pre-assigned number #1998 vibe-talk-github-helper-health-fallback. Depends on the error wording from #1997 vibe-talk-github-login-error-text. No host names, local user paths, or real account names in committed files; placeholders only.
