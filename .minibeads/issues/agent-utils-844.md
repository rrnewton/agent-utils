---
title: 'watch-notice-route: one notice per stuck episode, never delivered into the stuck agent''s own queue'
status: open
priority: 0
issue_type: bug
assignee: cl-start
created_at: 2026-11-04T18:54:30.514510772+00:00
updated_at: 2026-11-04T18:54:30.514510772+00:00
---

# Description

The lead (2026-11-04): before #840 watch-notice-backoff was deployed, the watcher queued 795 "agent-watch-owner-stuck-NAME-*" notices, one a minute, into the queue of the very coordinator whose pane was stuck. Those notices then blocked that queue themselves (one sat pasted in the stuck composer) and starved 27 real agent-teams messages. #840 adds backoff, timestamps and a single pending copy, but it still delivers the notice into the pane whose stuck composer it is about.

# Design

- One notice per stuck episode: create it once, then update or replace it in place (a tombstone plus one live copy at most). Never queue a new copy while the previous one is undelivered.
- Never route the notice into the agent whose pane or composer is the subject. Send it elsewhere: the registry's configured liaison or owner chat outbound (the bridge's alarm path), or another configured coordinator. If no alternative route exists, log it and raise a single owner alarm; do not queue it to the stuck agent.
- An episode ends when the owner messages are delivered (or the reason changes). The next episode starts fresh.
- Audit the other watcher notices for the same self-addressed pattern and apply the same rule.

# Acceptance Criteria

- Fake-clock tests: a 2-hour stuck episode produces exactly one live notice (updated in place) and zero items in the stuck agent's queue; an alternative route receives it; with no route, there is one owner alarm and no queue item.
- Gate passes; lands alone (P0).

# Notes

P0 by the lead, following #840 watch-notice-backoff. Assigned to cl-start (the #830/#840 author); #842 wrkviz-muse-backend-rust waits.
