---
title: 'watch-notice-route: one notice per stuck episode, never delivered into the stuck agent''s own queue'
status: closed
priority: 0
issue_type: bug
assignee: cl-start
created_at: 2026-11-04T18:54:30.514510772+00:00
updated_at: 2026-11-04T19:09:57.261125000+00:00
closed_at: 2026-11-04T19:09:57.261125000+00:00
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

## Resolution

[opus 5.5] cl-start, 2026-11-04 (rs/agentctl/src/agent_watch.rs).

- **Route, never self-addressed.** `notice_route(subject, jammed, options)` picks the coordinator, else each new `--notice-fallback NAME` (repeatable) in order. It never picks the agent the notice is about, nor any agent whose own queue is stuck or jammed in this pass. With no agent left, `alarm_owner` logs the notice and raises one owner alarm through the new `--owner-alarm-bridge DIR`: `chat_service::publish` posts it to the bridge's first channel with its outbound helper, and without that flag the alarm is only logged.
- **Owner-stuck notices:** one per episode.
  - The episode records where its live copy waits (`recipient`) and whether its alarm was raised (`alarmed`).
  - A repeat withdraws the live copy first. If that copy is `inflight` (being typed), or the withdrawal fails, no second copy is queued that pass.
  - With no agent route, the episode raises one alarm; a failed alarm is retried on the backoff, and a successful one is never repeated.
  - An episode that ends, because its requests were delivered, withdraws its live notice. A change in the kind of reason starts a fresh episode.
- **Audit of the other notices:**
  - Delivery jams, unattributed or swept editor drafts, and probable-misroute quarantines now go through the same route (`tell_about`). Before this, a jam on the coordinator's own lock, or a draft in its editor, was queued to the coordinator.
  - The held-queue escalation (#834) already told local senders about the coordinator's own queue. It now also tells the fallback, skips every queue held in this pass, and raises the owner alarm when no agent route exists.
  - Episode batches (approval prompts, idle, dead and so on) never concern the coordinator, which is not watched, and go to managers that are not themselves blocked, so they are unchanged.
- **Fake-clock tests:**
  - A 2-hour stuck coordinator leaves one live notice, with the fallback and none in its own queue.
  - With no route, 2 hours give one owner alarm and zero queue items; a failed alarm is retried, then never repeated.
  - A lane stuck beside a stuck coordinator goes to the fallback, and with the fallback stuck too, each episode alarms once.
  - No copy is queued beside one being typed.
  - An ended episode withdraws its notice.
  - The route skips the subject and jammed agents.
  - A held coordinator queue reaches its fallback; the jam on the coordinator's own lock reaches its fallback.

Deploy note: give the lead's watcher `--notice-fallback <another coordinator or liaison>` and `--owner-alarm-bridge <bridge state dir>`. Without them, a notice about the coordinator's own queue is only logged.

