---
title: 'chat-links-issue-label: rendered issue references show the raw URL or tag instead of #N'
status: open
priority: 2
issue_type: bug
labels:
- agentctl
created_at: 2026-11-16T12:40:06.000000000+00:00
updated_at: 2026-11-16T12:40:06.000000000+00:00
---

# Description

[watermelon-full-preview] In vibe-talk, a reply's rendered `:issue:658` shows as the full raw URL (an `https://…/agent-utils-658.md` address with a link arrow) rather than the short `#658` label. The bridge emits Slack's `<url|:issue:658>` form — the label is the written tag — and vibe-talk's Slack mrkdwn conversion honours a `<url|label>` label, but its short-reference pass deliberately leaves text inside a link as written, and when the label does not survive the provider round trip the link degrades to the bare address. Thread and message references already emit short labels (`:thread:060`, `:email:1846`, which Slack draws as its emoji plus number); only the issue label is the raw tag.

# Design

Bridge side: a rendered issue reference emits `<url|#N>` so every consumer (Slack, vibe-talk, any mrkdwn reader) shows `#N`. The unresolved form stays the written `:issue:N` citation with its warning. Thread and message labels stay `:thread:NNN` / `:email:N` — already the short forms both renderers shorten. On the converted outbound path the bridge renders references as Markdown links (`[#N](url)`, `[:thread:NNN](url)`) so the helper's converter expands each exactly once — never Slack `<url|label>` text into Markdown input, which the converter's autolink pass reads as one raw address and roughly doubles. Owner decision 2026-11-16: the message splitting itself is not changed (a tiny lead part is the agent's text as written; no merge, no cap change). Whether vibe-talk should also shorten labels inside links is reported to the coordinator as a possible separate lane; no vibe-talk change here.

# Acceptance Criteria

- A reply citing `:issue:658` posts `<https://…/agent-utils-658.md|#658>` (template-dependent address, `#658` label).
- An unresolvable `:issue:N` stays plain `:issue:N` with its existing warning.
- Thread and message rendered labels are unchanged.
- On the converted path the rendered mrkdwn for a body with each reference kind holds one link each with a short label and no doubled address.

# Notes

Observed in vibe-talk on replies rendered by the #658 bridge. Bridge emits the `<url|label>` form today (not a bare URL); vibe-talk renders link labels (`slack/mrkdwn::token`) but never rewrites text inside a link.
