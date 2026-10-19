---
title: 'provider-grouped-channels: with several chat services in one server, group every channel picker by service and choose the service first in Settings'
status: open
priority: 2
issue_type: feature
labels:
- vibe-talk
created_at: 2026-10-19T08:28:09.877914416+00:00
updated_at: 2026-10-19T08:28:09.877914416+00:00
---

# Description

[opus 5.5] Owner, 2026-10-18: "what barriers are there to supporting multiple backends in one server? ... How hard would it be to have any subset of our {slack, gchat, discord, ...} providers active within any given vibe talk server instance? ... under the Channels setting ... drop-down for Service=Slack/etc and then within that a list of subscribed channels ... channel drop-down ... segmented/grouped ... PER service."

One server can already read several chat services at once (#179 slack-provider): each is a [[providers]] entry, every channel names its provider, and reads, posts, live updates and diagnostics go to that provider. What is missing is mostly on the page, which shows every channel as one flat list and treats Settings as one service, plus a few server gaps that only show with several services.

Wanted, when two or more services are configured:
- Settings > Channels starts with a Service picker. Below it are only that service's channels, a line saying what the service does here (how new messages arrive, threads, adding by link, browsing, clearing unread), and the service's own actions: Add a channel and Browse channels add to THAT service, and the hidden-channels list shows that service's hidden channels. Settings opens on the service of the channel being read.
- "All channels" in Settings' channel picker (for Clear unread) means all of the chosen service's channels, offered only for a service that can clear unread.
- The channel picker on the bar (and Ctrl+K, which opens it) groups its channels under a header per service, in configuration order. The closed picker shows the channel's name; the open list has the service headers. Alt+Up and Alt+Down step through channels in the order shown, across groups. No headers when only one service has channels.
- The channel browser lists only the chosen service's channels, and "Already added" is right for every entry.
- Adding a channel by id works with several services, even when none of them manages registration; today such an add is refused by the probe as "not assigned to any configured chat provider", and a direct add can lose its service as soon as it is stored.
- Adding from the app is refused, with a reason, for a second account of one service (a provider sharing its API base with an earlier configured one), because the channel would come back under the other account. Storing the account beside each added channel is a follow-up issue.
- The voice agent's channel list names each channel's service.

With one service, nothing changes: no Service picker, no headers, and the same requests.

Not on the page today, so nothing to group: there is no sidebar channel list and no search scope (search filters the open channel's loaded messages). The only channel pickers are the bar's and Settings'.

Not in scope (follow-ups if wanted): two accounts of one service adding channels from the app; merging the stores of two existing deployments into one server; provider-qualified channel ids; spoken service names.

Tests: server tests for the scoped browser, adding with a named service (including the existing direct-add failures), the second-account refusal and the voice agent's list; page tests with one and two services; a Chromium check of the grouped picker and the Settings Service row at phone and desk widths, and a screenshot state.
