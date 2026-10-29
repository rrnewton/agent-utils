---
title: 'muse-bridge-rescan-intake: Slack rescans hold live messages and spend the owner''s rate limit'
status: closed
priority: 1
issue_type: bug
created_at: 2026-10-29T15:54:23.496721519+00:00
updated_at: 2026-10-29T17:37:24.170343356+00:00
closed_at: 2026-10-29T17:37:24.170343136+00:00
---

# Description

[opus 5.5] Found by #679 muse-bridge-study; the yield below is fixed by #678 muse-bridge-latency's
`chat-subscription-slack: a rescan pass yields to a waiting live message`.

The Slack plugin's rescan (rs/chat-subscription-slack plugin.rs `rescan_if_due` -> `rescan_pass`)
runs synchronously in the driver loop every `rescan_interval_seconds` (90), and 20 s after any
stream-carried message (`ACTIVITY_RESCAN_MILLIS`). A pass lists the channel's last day of
history and up to eight threads, and since #615 waits out Slack's rate limits inside the pass.
Live events that arrived meanwhile waited for the whole pass: on the test channel, once it held
a few dozen threads, the bridge admitted (👀) goal-run messages 22-57 s after they were posted
(one forwarded the millisecond a pass logged a `conversations.replies` rate limit).

Fixed: after its first history listing a pass yields to a live event already waiting, between
listings and during rate-limit waits; the next pass is scheduled as usual. After the fix, 0.6-
1.8 s on the same channel.

Still open (why this issue stays open):

- Every bridge reads the channel as the owner, so its passes count against the owner's Slack
  rate limits; on 2026-10-29 the limiter began throttling the owner's history/replies reads
  while several bridges (including this study's test bridges) were rescanning, which slowed
  the owner's other Slack tools. A pass after every message (the 20 s pull-forward) lists
  threads too. Consider: pull-forward lists only history (file messages are top-level or
  thread broadcasts in it), threads only on the 90 s cadence; a shared rate budget across
  bridges on one host; and a backoff when the limiter answers.
- In a channel that never goes quiet for a whole pass, yielding postpones file-message rescue
  until it does.
