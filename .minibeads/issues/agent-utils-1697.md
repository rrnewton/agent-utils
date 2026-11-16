---
title: 'slack-file-download-through-vibe-talk: fetch a shared file once, through the server'
status: open
priority: 2
issue_type: task
labels:
- slack
- vibe-talk
created_at: 2026-11-16T02:00:00.000000000+00:00
updated_at: 2026-11-16T02:00:00.000000000+00:00
---

# Description

When a message with an attached file reaches an agent through the chat bridge, the delivered note instructs the agent to download the file itself with a Slack client command, one direct fetch per agent per file. vibe-talk never fetches the file at all; it renders only a link. So file access has no cache, no shared budget, and a full client run on every agent that looks at a screenshot.

# Design

vibe-talk fetches a shared file once, caches it behind its existing per-file link, and serves it to authorised readers. The bridge's delivered file note and the agent-facing guidance name that server endpoint instead of a Slack client download command. A file the server has not fetched yet is fetched on first request, once, however many agents then read it.

# Acceptance Criteria

- Two agents reading the same shared file cause exactly one upstream file fetch.
- The bridge's file note and the agent guidance name the vibe-talk endpoint, and no agent-facing text instructs a direct Slack file download.
- A file that cannot be fetched is a loud error at the endpoint, not a silent empty download.

# Notes

[watermelon-full-preview] Filed by the direct-Slack burn-down enumeration (owner request 105, 2026-11-15): the target model is client <> vibe-talk <> Slack, and this is one remaining path that reaches Slack without vibe-talk's cache or writer in between. Rates and the shared-budget accounting are in #1682 slack-rate-limit-budget.
