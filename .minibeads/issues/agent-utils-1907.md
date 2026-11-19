---
title: 'agentctl-chat-trace-v2-sent-refs: bridge carries pending sent refs on v2 trace entries'
status: open
priority: 0
issue_type: feature
labels:
- agentctl
created_at: 2026-11-19T08:17:33-05:00
updated_at: 2026-11-19T08:17:33-05:00
---

# Description

[watermelon-full-preview] Owner feature #1886 vibe-talk-transcripts-view (Transcripts), bridge side. Per the Bridge v2 `sent` spec in #1886 Notes (2026-11-19): the chat runtime records each chat message it posts for an agent (thread_ts, its ts, char count) as pending sent refs. The next trace entry it posts carries v:2 and sent:[...]: at most 8 refs, oldest first; drop the OLDEST until the serialised payload fits in 2 KB; never truncate or split a ref. Roots stay kind:"root" with no sent. v1 readers keep working (the consumer accepts v1 and v2). Keep the metadata shape {event_type:"agentctl_trace", event_payload:{...}} under 8 KB.

# Design

- In `rs/agentctl/src/chat_trace.rs`: a `SentRef` {thread, msg, ts, chars}, `trace_metadata_with_sent` (roots keep the v1 shape with no `sent`; entries are v:2), and fitting that caps at 8 refs then drops the oldest until the whole serialised event_payload fits in 2 KB, never truncating or splitting a ref. The existing `trace_metadata` keeps its v1 shape for roots and legacy callers.
- Pending refs are a new persisted bridge-state file `pending-sent.json` in the state root (next to `publishes.json`): a versioned list capped at 64, oldest dropped past the cap. Reply delivery (`publish_one`, including its top-level fallback, whose thread is its own ts) records a ref after a successful send; trace posts and operator `chat publish` posts are not agent replies and never record. Older pinned bridges ignore the unknown file, so no existing persisted field changes shape.
- The trace driver (`spawn_trace_driver`) claims pending refs only for the first part of an entry post, stores the claimed fitted refs on the held post so a retry carries the same refs, and never claims for a root. A failed-and-dropped post discards its refs; a dropped-for-budget ref simply never interleaves, per spec.

# Acceptance Criteria

- Payload shape and budget tests: 8-ref cap, 2 KB drop-oldest, a single ref never split, refs in send order, roots unchanged, v1 fallback (a v2 entry with no sent and v1 metadata both class on the consumer side).
- An end-to-end dynamic test with a fake Slack-shaped transport: the bridge posts a reply, then a trace entry; the trace entry's sent[] names that reply, and the consumer's sent-refs parser reads it back.
- Focused agentctl tests pass; the trace metadata stays {event_type:"agentctl_trace", event_payload:{...}} under 8 KB.

# Notes

Related: #1886 vibe-talk-transcripts-view parts A-C (consumer `vibe-talk/src/transcripts.rs` already parses v2 `sent`, caps at 8, and refuses payloads over 2 KB), #1884 vibe-talk-thread-naming-from-cache, #832 chat-trace-format-tagging. Reader skew: pinned bridges run older builds; the only new persisted state is the `pending-sent.json` file named under Design, reported to the coordinator before gating. Test only against a fake Slack or test channel, never a real owner channel.
