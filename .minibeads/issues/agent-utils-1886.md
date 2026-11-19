---
title: 'vibe-talk-transcripts-view: transcript threads hidden everywhere but Transcripts, which opens newest and windows back'
status: open
priority: 0
issue_type: feature
labels:
- vibe-talk
- ui
created_at: 2026-11-18T22:30:00.000000000-05:00
updated_at: 2026-11-18T22:30:00.000000000-05:00
---

# Description

[watermelon-full-preview] Owner threads 212-214 and 218 (2026-11-18). Step 1 of #1608 agent-transcripts-timeline landed the Transcripts view and server timeline exclusion, but the owner still sees a fresh daily trace root ("<agent> main session trace, <date> ET", a new thread each midnight ET) in Main after a reload, transcript arrivals spam the raw views live, and the Transcripts view itself loads a whole day slowly from the top.

Part A (landed first, on its own): transcript threads, including the daily root post and not just replies, never appear in Main, All, two-column or thread views, and never count as unread or notify. No checkbox. The remaining parts, as follow-ups: (B) the Transcripts view opens at the newest entry, follows new entries, and demand-loads older ones windowed, with no full-history load and no placeholder flash; (C) the agent's sent chat messages interleave where they happened as small vestigial entries tinted with their thread colour, expanding to the full card; and the addendum-1 read-path work (transcript and older reads cache-only and never blocking on upstream, view switches never waiting on an in-flight older read, Cmd-2 and the other view shortcuts working from the Transcripts view).

# Acceptance Criteria

- A transcript thread (trace metadata on root or entry, or the fixed root title), including a fresh daily root with no replies yet, is absent from Main, All, two-column and thread views on every delivery path: timeline reads (upstream and cache-served), the live stream and its replay tail, live folds into the page's store, and a store restored from the device's saved copy.
- Transcript messages never count as unread, never raise the jump-to-newest offer, and are never relayed to a call.
- The Transcripts index still lists the agent-day and its day still opens from the transcripts route and from a search hit.
- Focused tests through the real router with a seeded transcript thread, a browser test with a live transcript arrival, and the standing screenshot matrix on a test instance.

# Design

[watermelon-full-preview] Part A root cause, found by reproduction: the server timeline route already excludes classed messages (a router probe with metadata and title-only roots passes for Main/All/Threads/Thread, upstream and cache-served), but the live SSE stream (`live::events`/`all_events`) emits every published message with no transcript check, and the page folds every live arrival into its channel store with no class check either. A fresh root therefore arrives live, is saved in the device's channel copy, and is replayed from the stream tail on reload — which is why a reload still showed it. The fix closes all three: the stream never emits a transcript message (and records its class for the index), the page drops transcript arrivals before they fold, count, jump or relay, and a restored saved store drops transcript rows it was saved with.

# Notes

[watermelon-full-preview] 2026-11-18. Part B/C and addendum-1 follow as separate changes after part A lands.

[watermelon-full-preview] 2026-11-19. Part A LANDED as 7d94969e on main (gated 571f4d1b; flake waiver on typing-scroll-browser and screenshots-desks, both load/timing failures on untouched steps, each passed its exclusive rerun). Root cause of the fresh daily root in Main after a reload: the timeline route already excluded classed messages, but the live hub/streams published transcript messages unchecked and the page folded and saved them unchecked; all three now keep transcript threads — root included — out of Main, All, two-column and thread views, unread counts, the jump offer and the call relay. Remaining open: part B (newest-first windowed Transcripts view with follow and demand-loaded older pages), part C (vestigial interleaved sent-chat entries, expandable), and the addendum-1 read-path work (cache-only transcript/older reads, view-switch abort, Cmd-2 from Transcripts), as separate changes.

[watermelon-full-preview] 2026-11-19. Bridge v2 `sent` spec (for the separate agentctl lane; vibe-talk consumes this in part C, option 1). Trace metadata stays Slack message metadata `{event_type: "agentctl_trace", event_payload: {...}}` (the devbig014 shim accepts only that top-level shape, whole metadata under 8 KB). A v2 entry payload is `{v: 2, kind: "entry", agent, team, session, day (ET YYYY-MM-DD), seq, trigger, sent: [...]}`; roots keep `kind: "root"` and no `sent`. Each `sent` item names one chat message the agent sent, in send order: `thread` = the Slack `thread_ts` it was sent into (its own `ts` when top-level), `msg` = the sent message's Slack `ts`, `ts` = the same instant as `msg` (ordering when the message itself cannot be read back), `chars` = the sent text's character count. Example: `{"event_type":"agentctl_trace","event_payload":{"v":2,"kind":"entry","agent":"liaison","team":null,"session":"s1","day":"2026-11-19","seq":7,"trigger":"owner message","sent":[{"thread":"1763510400.000100","msg":"1763510400.000100","ts":"1763510400.000100","chars":212},{"thread":"1763509900.000200","msg":"1763510460.000300","ts":"1763510460.000300","chars":96}]}}`. Budget rule (shim 8 KB, and readers refuse a payload over 2 KB, so the binding budget is 2 KB for the whole serialised payload): the bridge attaches its pending sent refs to the next trace entry it posts, at most 8 refs, oldest first; if serialising them would pass 2 KB it drops the OLDEST refs until it fits (never truncates a ref, never splits one across entries) — a dropped send simply never interleaves, and the entry still classes its thread. Readers accept v1 and v2 forever: v1 (or a v2 entry with no `sent`) means no timeline cards, never no class. vibe-talk resolves each ref from its own cache copy of the channel (no upstream read on the transcripts path); an unresolved ref is skipped, not faked.
