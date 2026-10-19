---
title: 'live-reactions: a reaction added in the native app shows in vibe-talk within seconds'
status: closed
priority: 2
issue_type: feature
labels:
- vibe-talk
created_at: 2026-10-19T19:00:32.911813941+00:00
updated_at: 2026-10-19T19:53:21.093852917+00:00
closed_at: 2026-10-19T19:53:21.093852507+00:00
---

# Description

Behaviour wanted: when somebody adds or removes an emoji reaction to a message in the provider's native app, an open vibe-talk page showing that message (channel or thread) shows the new reaction set within a few seconds, without the reader reloading or re-opening anything. Same emoji (a custom emoji by its name where no image is available), same counts, and the owner's own reaction marked as the native apps mark it.

Today (#219 emoji-reactions, #312 slack-reactions) vibe-talk shows the reactions a message carries when it is read. A reaction added to an already-read message is never picked up live: the live poller only publishes messages newer than its cursor, so a thumbs-up added in the native app to a message already on screen never appears. Example from the owner (2026-10-19): a thumbs-up added in the native Slack app never showed in vibe-talk.

Rules:
- Where a provider or bridge pushes message-change events, a reaction change rides the live stream as a message update carrying the message's full current reaction list.
- Where a provider only reports reactions in history, vibe-talk re-reads a bounded recent window of a watched channel on its poll cadence, diffs the reaction sets, and publishes an update for each message whose reactions changed. No cached copy (held window or message cache) may serve reactions older than that refresh once it has run.
- Reactions stay read-only in vibe-talk, as recorded for #219: the owner does not react from vibe-talk.
- A copy that says nothing about reactions still never removes chips a read brought.

What a private bridge must emit for live reactions:
- Discord-protocol bridge, pushed events: POST /api/v1/live/events with kind update (a fresh stable event id per change), the full message object, and its reactions array in the Discord shape: [{count, emoji: {id: snowflake-or-null, name: emoji-or-custom-name}}]. An empty array means the last reaction was removed; omitting the field means 'not reported' and changes nothing. Alternatively, or as well: POST /api/v1/live/hints for the channel when any message in it changes, reactions included, and serve the current reactions array on the message list the following poll reads.
- Slack Web API shim: include reactions: [{name, count, users}] on every message conversations.history and conversations.replies return (empty/absent as today when none or unknown), so the newest-page read the live poller already makes carries the current sets. For sub-poll-interval latency, also POST /api/v1/live/hints for the conversation when a reaction changes; the hint is content-free and only wakes the poller.

Tests: fake providers show a pushed reaction on the stream and a history-only reaction picked up by the refresh; the held window and cache never hide a newer reaction set; a browser check shows the chip appear on an open page without a reload. Latency from native reaction to display is measured with fakes and reported.
