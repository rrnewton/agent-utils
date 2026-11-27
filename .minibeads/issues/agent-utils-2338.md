---
title: 'chat-bridge-reply-window-retire: chat bridge requests never retire because reply windows never close, so the bridge fills at its request limit and refuses new owner messages'
status: open
priority: 0
issue_type: bug
assignee: cl-bridgeretire
labels:
- agentctl
- chat
- bridge
created_at: 2026-11-26T21:05:00-05:00
updated_at: 2026-11-26T21:05:00-05:00
---

# Description

The chat runtime keeps one persisted record per admitted chat request and caps the population at `MAX_REQUESTS` (2048). A record retires (frees its place) only when it is delivered, acknowledged, every reply is terminal, and `reply_closed` is true. `reply_closed` is set only by `agentctl chat close --request KEY`, and nothing calls it in normal operation, for a headless coordinator or an interactive one. So every answered request stays forever, and a busy bridge reaches the limit.

Observed on 2026-11-26: a bridge serving a headless coordinator reached 2048/2048 with "chat request population limit reached; no safely retired request can free enough capacity". Of its records, 2047 were delivered, acknowledged, with every reply sent, and `reply_closed=false`; one was `delivery_uncertain`. Twelve owner messages got no acknowledgement and no delivery. A second bridge (interactive target) had 393 records, all with `reply_closed=false`; 366 old answered ones were closed by hand with a script to free space.

The failure is silent to the owner: their message is simply not acknowledged.

# Design

- Close the reply window at a well-defined point without a hand close: an answered request (at least one reply, every reply terminal, nothing pending) closes once a later request of the same conversation has been delivered, or a bounded quiet age after its last reply. An unanswered request (no reply yet) or one with a reply still pending is never closed by these rules, except a long age bound after admission (well beyond any reply reminder window), so a headless reply reminder still sees it.
- When the population limit is reached, retire eligible requests oldest first (including the age-bound closes) before refusing; if still full, raise a loud alarm: an error log line, `chat health` reports the bridge as not healthy with the count, and a notice the agent watcher surfaces, rather than leaving owner messages silently unacknowledged.
- A held or refused inbound event is not consumed: the event cursor does not move past it, and it is admitted once space frees.
- Times come from an injected clock in the runtime so tests are deterministic.

# Acceptance Criteria

- An answered request retires without `chat close` once the defined point is reached; an unanswered or mid-reply request does not.
- A fill-to-limit test through the real chat runtime: the population at the limit, retirement frees space, and the next owner message is admitted and delivered; with nothing eligible, the event is held (cursor not advanced), the alarm is raised, and the event is admitted after space frees.
- No wall-clock budgets in tests (owner rule 346): clocks are injected.
- `chat health` reports a full or nearly full request population.

# Notes

Reported 2026-11-26 by the lead coordinator for the owner ("it can silently drop the owner's input"). Coupled with `#2331` (headless reply reminders): this change must not close or retire an unanswered request at turn end; whichever lands second rebases.
