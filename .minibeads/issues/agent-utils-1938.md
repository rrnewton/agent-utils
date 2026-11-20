---
title: 'hooks-dialog-real-capture-answer-test: drive the exact real capture through the answer path'
status: open
priority: 2
issue_type: task
labels:
- agentctl
created_at: 2026-11-20T00:00:00+00:00
updated_at: 2026-11-20T00:00:00+00:00
---

# Description

[watermelon-full-preview] From the third review of #1932 (agentctl-muse-hooks-trust-auto-decline, landed 62f03e1d): the exact live capture (Muse 1.4.25, 23 hooks, startup banner, eleven blank lines, `⟩` numbered rows) is a fixture checked for recognition in both editions, and the fake-dialog renderers draw the real form, but no committed test drives the capture's exact bytes through `answer_hooks_trust_dialog` end-to-end (Down/Down/Enter for decline, Down/Enter for trust, composer reached).

# Design

- Add an answer-path test in each edition whose pane serves the exact capture bytes and records keys: decline confirms option 3, trust (explicit policy) confirms option 2, and no Esc is sent.

# Acceptance Criteria

- Both editions have a committed test answering the exact capture under decline; one also under trust.
- The recorded key sequence contains no Esc and no extra keys.

# Notes

Follow-up to #1932 agentctl-muse-hooks-trust-auto-decline; not a landing blocker for it (review verdict SAFE).
