---
title: 'agent-teams-relay-slack-link-markup: Slack rewrites domain-like tokens inside the relay envelope, breaking the body hash'
status: closed
priority: 1
issue_type: bug
assignee: mu-mirror@devbig014
labels:
- agent-teams
created_at: 2026-11-02T22:54:03.979871738+00:00
updated_at: 2026-11-02T23:56:01.375892782+00:00
closed_at: 2026-11-02T23:56:01.375892682+00:00
---

# Description

[opus 5.5] 2026-11-02: root cause of body-hash-mismatch stranger alarms in both directions: Slack rewrites domain-like tokens INSIDE the fenced envelope into link markup. Example: 'cli.rs' comes back as '<http://cli.rs|cli.rs>' (.rs is a TLD); the same happens to URLs ('<https://...>') and email addresses ('<mailto:...|...>'). relay-in unescapes only &amp;/&lt;/&gt;, so the body hash fails. Many real messages name files like x.rs or x.md, so many mirror copies fail; direct delivery is unaffected. Fix: (a) the sender JSON-escapes '.', ':' and '@' (and < > &) inside string values when rendering the fence (identical JSON when parsed, so hash and signature hold; budget for the growth in max_post_bytes); (b) the receiver, on a hash failure, retries after undoing Slack link markup inside the fence (<http://X|X> -> X, <https://X|X> -> X, <mailto:X|X> -> X, <URL> -> URL) and accepts only if hash and signature verify; (c) a body-hash mismatch from a CONFIRMED sender goes under the dismissable unparseable kind, never stranger. Meanwhile mirror stays on and hosts clear the alarm with 'relay confirm' of the shared bot.
