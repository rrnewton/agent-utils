---
title: 'hooks-dialog-require-count-line: shortened dialog forms without the hook-count line are accepted'
status: open
priority: 2
issue_type: task
labels:
- agentctl
created_at: 2026-11-20T00:00:00+00:00
updated_at: 2026-11-20T00:00:00+00:00
---

# Description

[watermelon-full-preview] From the third review of #1932 (agentctl-muse-hooks-trust-auto-decline, landed 62f03e1d): `_hooks_trust_block` / `hooks_trust_block` accept dialog forms with both body lines missing, or a bare title directly above the options, because the between-title gap check allows an empty gap and the count line is optional in the parse (it is only extracted when present). Real Muse always draws the count line ("N new or changed hooks need your trust."), so requiring it tightens the match at no liveness cost and also hardens the scrolled-off quote case (#1948 hooks-quote-scrolled-marker).

# Design

- Require the hook-count line between the title and the option rows in both editions, identically; keep the log's "hook count not shown" path only for surfaces that never answer (status naming keeps the loose recognizer).

# Acceptance Criteria

- A dialog form with no count line is not answered in either edition; the real Muse 1.4.25 capture still is.
- Both editions' parsers stay byte-identical in acceptance on the shared scenario table.

# Notes

Follow-up to #1932 agentctl-muse-hooks-trust-auto-decline; not a landing blocker for it (review verdict SAFE).
