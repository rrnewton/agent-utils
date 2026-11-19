---
title: 'agentctl-chat-owner-channel-starved: the relay poll starves the owner rescan, and chat health stays ok'
status: closed
priority: 1
issue_type: bug
labels:
- agentctl
- chat
created_at: 2026-11-19T04:55:00.000000000+00:00
updated_at: 2026-11-19T05:40:00.000000000+00:00
closed_at: 2026-11-19T05:40:00.000000000+00:00
---

# Description

[watermelon-full-preview] Reported by a coordinator on another host (trusted, 2026-11-18 11:46 PM ET): that host's chat bridge (snapshot 0a6f12da) has been
effectively deaf to the owner since 2026-11-12 06:28 ET (last admitted owner request), yet `chat health` says verdict ok.

Two symptoms in the plugin diagnostics log:

(a) The live stream carries nothing: every relay-channel message shows as "found N messages the stream did not carry", rescued by the relay poll every ~30 s.

(b) The owner channel only gets the 90 s rescan, and the log repeats "listing rate-limited: conversations.history in channel <relay> ... Retry after N s" then "rescan: the host listing budget holds every bridge back after a rate limit after 0 listings; next pass in 90 s". The relay poll's history calls exhaust the Slack listing limit, and the shared host listing budget's back-off then holds the owner-channel rescan back at 0 listings, pass after pass.

So the relay/mirror poll starves the owner channel: it runs first and often (30 s cadence), spends the shared per-minute budget, and when Slack rate-limits it, the back-off it records blocks the owner rescan too. `chat health` has no signal for "nothing admitted for days while the stream carries nothing" or "owner rescan held back", so it reports ok throughout.

Prior work: #690 muse-bridge-rescan-intake (host-wide listing budget), #615 chat-file-messages (in-pass rate-limit retry), #769 relay-rescan-settle-latency (relay poll), #805 relay-poll-receipts-during-hold, #1682 slack-rate-limit-budget (relay poll floored at 30 s, half-budget headroom for the general rescan).

# Design

- Each general rescan pass lists the owner (non-relay) channels first and relay channels last, so a pass that runs out of budget or hits a rate limit has already given the owner channel its listings. The relay poll keeps its own smaller listing budget (its own budget file, capped below the owner rescan's allowance) and a rate limit or back-off it causes holds only the relay poll, never the owner rescan. The relay poll's existing half-budget headroom for the general rescan stays.
- `chat health` reads the plugin diagnostics log the bridge already writes (no new persisted bridge-state field) and reports degraded, not ok, when: no owner request has been admitted for the silence threshold while the log shows the stream carrying no request in that span but the rescan still finding messages the stream did not carry; or the owner rescan has been held back by the listing budget for longer than the hold threshold. Both signals come from lines existing bridges already write, so a pinned older bridge is judged the same way.
- Investigate why the live stream stopped carrying events on the reporting host (event subscription, identity the tail runs as, cursor) from the code's own diagnostics and report the finding under Notes; the code fix must not depend on reaching that host.

# Acceptance Criteria

A fake-Slack test with a busy relay channel and a quiet owner channel admits the owner message within one pass even while the relay poll is rate-limited; the relay poll's back-off never holds the owner rescan at 0 listings; `chat health` reports degraded with the starved-owner reason on a state whose last admission is past the threshold and whose stream carried nothing, and stays ok on a healthy state; existing rescan, relay and health tests stay green.

# Notes

## Resolution

Fixed in this lane. (1) `rs/chat-subscription-slack`: each general rescan pass now lists non-relay (owner) channels before relay channels, and the relay poll spends its own smaller host budget (`CallBudget::for_relay_identity`, half the configured per-minute allowance, in `budget-relay.json` beside the owner budget) — a rate limit or back-off the relay poll causes holds only the relay poll, never the owner rescan; the #805 held-poll receipt listings and half-budget stop now apply to that relay budget. (2) `agentctl chat health` now reports degraded (not ok) when no owner request has been admitted for 48 h while the plugin diagnostics log shows the stream carrying no request in that span but the rescan still finding messages the stream did not carry, and when rescan budget-hold lines span more than 30 min with a fresh latest line; the report gains an `owner_channel` JSON section. No new persisted bridge-state field: both signals parse lines bridges already write (`event N: request ...`, `... the stream did not carry`, `rescan: the host listing budget holds ...`), so a pinned older bridge is judged identically — coordb was told before gating. Evidence: new fake-Slack test `a_rate_limited_relay_poll_never_starves_the_owner_rescan` (busy relay channel rate-limited on every listing, quiet owner channel: owner file message admitted within one pass; back-off present only in `budget-relay.json`), plus health tests `a_deaf_bridge_with_a_silent_stream_is_degraded_not_ok` and `an_owner_rescan_held_by_the_budget_is_degraded_not_ok`. The two #805 hold tests and the #769 headroom/refusal tests were moved to the relay budget file, and the two hold tests that shared one test clock were split onto separate clocks after intermittent parallel failures.

## Why the live stream carries nothing (investigation, item 3)

Not fixed in code, because the code already names the likely causes and the fault is in that host's configuration, which this lane cannot reach. Reading `rs/chat-subscription-slack/src/plugin.rs` and `cli.rs`: the stream carries channel events only for the identity the client runs as — Slack delivers channel message events to a human member's own subscription, not to an app's Crewmate (#390 slack-inbound-identity; the log warns once when the client's stderr names a Crewmate, and notes once when a stream stays subscribed 30 min with no event at all). Relay-channel bot posts are never carried by design (that is what the relay poll is for), so "found N messages the stream did not carry" for the relay channel alone would be normal; the deafness is that owner-channel posts were also only ever rescued. A fallen-out-of-retention cursor would instead declare a gap and stop the bridge (health down, not ok), and the channel set the stream subscribes to is the same set the rescan lists, so neither explains a live-but-silent stream. Most likely, in order: the tail on that host runs as a Crewmate or as an identity that is not a member of the owner channel, so Slack delivers it no channel events; or the client's event subscription does not include that channel. On that host, check `plugin.log` for the Crewmate warning and the 30-minute quiet notice, and confirm `cli_path` runs as a member of the owner channel. Until then the rescan is the only owner path, which is exactly why the starvation fix above matters.
