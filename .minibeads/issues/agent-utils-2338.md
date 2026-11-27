---
title: 'chat-bridge-reply-window-retire: chat bridge requests never retire because reply windows never close, so the bridge fills at its request limit and refuses new owner messages'
status: closed
priority: 0
issue_type: bug
assignee: cl-bridgeretire
labels:
- agentctl
- chat
- bridge
created_at: 2026-11-26T21:05:00-05:00
updated_at: 2026-11-27T15:38:17-05:00
closed_at: 2026-11-27T15:38:17-05:00
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

## Resolution

Fixed. Landed as dbfbb877 on 2026-11-27 (74cb54e8, 57b0ffc6, dbfbb877), replaying ec2cf443.

**Windows.** `chat run` closes reply windows by itself, and a sweep runs every 10 minutes.
- An answered request closes 24 h after its last reply, and a later reply restarts that window. Closing on the next delivered request of the same conversation, as the design first proposed, was dropped: a progress reply within the day must still post.
- An unanswered request closes 48 h after admission, well past the reminders' 24 h window. A turn's end never closes it, and once closed it is never reminded, which was the coupling with #2331.
- A request whose prompt is pending, being submitted or uncertain, or which has a reply not yet sent, never closes by these rules.
- The windows are set in `reply_windows` in the bridge configuration.

**Full store.** At the 2,048 limit, answered requests close oldest last reply first, then unanswered ones admitted over 24 h ago.
- An unanswered request is never closed silently. It is listed in `reply-window-closures.json`, logged, sent to the agent in a queued notice and sent to the owner in an owner-alarm notice, and each step is recorded once.
- If nothing can close, admission refuses: the provider batch stays uncommitted and the cursor does not move. `request-capacity.json` records the hold, `chat health` is DOWN, the log has an error line, and the owner alarm warns hourly and posts a resolved line when the hold clears.
- From 90% full, the agent is warned at most once an hour and `chat health` is DEGRADED. The agent hears an all-clear once the store falls below 80%.

**Health.** `chat health` also sees a runner in another PID namespace, through a shared-lock probe on the lease. The owner-rescan budget warning counts only a continuous hold.

**Tests.** The sweep and admission take the time as an argument, so the tests use explicit times and no wall-clock budget.
- `a_full_population_retires_answered_requests_first_then_holds_new_messages_until_space_frees`: the fill-to-limit case through the real runtime.
- `an_answered_request_closes_and_retires_a_day_after_its_last_reply_and_not_before` and `pending_uncertain_and_mid_reply_requests_never_close_however_old`.
- `a_request_the_sweep_retires_is_never_reminded_afterwards`, with #2331 reminders on.
- `a_full_request_store_is_down_and_a_nearly_full_one_degraded`.
- Restart and crash cases: `a_sweep_cut_short_by_a_crash_finishes_after_the_restart_and_lists_each_request_once` and `every_request_admitted_before_a_restart_survives_it_and_the_first_sweep`.

**Evidence: gate `1795791500839228085-3fd2c9ce` on ec2cf443.** Path-selected validate, 54 steps, verdict pass, with failed, incomplete and known-red all empty.
- Main had changed `embedded_chat_userguide.md` since the gated base (#2366), so the replay landed with `--allow-overlap` on a combined tree verified for that docs-only overlap.
- On that tree: the user guide was read, fmt, the docs checks, the guide, reply-window and reminder tests, and `chat_reply` all pass.
- The new persisted fields were reported to the downstream team before landing, with no objection: `reply_windows` in the bridge configuration, `capacity` in `owner-alarm.json`, and the new `reply-window-closures.json` and `request-capacity.json`.
