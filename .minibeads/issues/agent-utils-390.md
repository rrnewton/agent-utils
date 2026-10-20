---
title: 'slack-inbound-identity: the Slack recipe implies one client identity for both directions, but channel events reach only the human member'
status: closed
priority: 2
issue_type: bug
labels:
- agentctl
created_at: 2026-10-20T07:47:56.367314566+00:00
updated_at: 2026-10-20T07:53:17.949094508+00:00
closed_at: 2026-10-20T07:53:17.949094328+00:00
---

# Description

[opus 5.5] The Slack provider guide configures one client for the plugin's tail (cli_path) and the outbound helper (--cli), which implies one identity serves both directions. In practice, on 2026-10-19, channel message events were delivered to the human owner's own event subscription and not to the registered app's Crewmate: a tail run as the Crewmate subscribed, heartbeated and stalled/revived hourly, and received 0 events for hours, while the same tail run as the owner caught up the waiting message at once. Replies must still post as the app. Separately reported, not a bridge defect: the message reached the bridge about 200 s after it was posted (message ts 1792457410.53, 00:50:10Z; fetched 1792457647.97, 00:54:08Z, per plugin.log). Caveat: plugin.log shows it arrived in the catch-up of a tail restarted at 00:54:02Z without a saved cursor, the restart that switched the tail to the owner's identity, so most of that interval may be the restart rather than provider latency; no live-stream latency was measured.

# Acceptance Criteria

The guide says the tail must run as a member that receives the channel's events (not an app's Crewmate) while the outbound helper may run as the app, and that cli_path and --cli are separate client paths for that reason; the plugin logs a warning when its tail's output shows it runs as a Crewmate, and a one-time notice when a stream stays subscribed with no events; the reported 200 s delivery delay is recorded with its caveat.
