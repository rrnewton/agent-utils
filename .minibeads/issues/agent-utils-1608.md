---
title: 'agent-transcripts-timeline: a Transcripts view for agent main-session traces, with search hits behind the Agent Transcripts bucket'
status: open
priority: 1
issue_type: feature
labels:
- vibe-talk
- agentctl
created_at: 2026-11-17T12:00:00.000000000-05:00
updated_at: 2026-11-17T12:00:00.000000000-05:00
---

# Description

[watermelon-full-preview] Owner 101 (2026-11-15), owner 141 and owner 144-146 (2026-11-17). Design: `ai_docs/plans/2026-11-03-vibe-talk-transcript-threads-design.md` (Owner 101 revision). The bridge already posts each agent's non-reply transcript text to a daily trace thread (#714 chat-transcript-primary, #832 chat-trace-format-tagging). Today those posts sit in the raw feed disguised as ordinary threads. Owner 101: transcript posts never enter the raw message feed at all; they live only in a new top-level Transcripts view (agent · day, no `slug: description` thread naming ever), with a timeline interleaving transcript chunks and the chat messages actually sent, a lower Slack post rate, and a vibe-talk on/off toggle that travels as an owner-identity bridge control message. Owner 141 adds: search hides transcript hits by default; the search bucket row (owner 145/146, #1797) carries an "Agent Transcripts" toggle chip, off by default except from the Transcripts screen; enabling it interleaves transcript hits chronologically, and tapping a hit opens the Transcripts view at that message, in context.

Build is staged (design section 8). Step 1 (this issue's first landing): classification, the Transcripts view, the search bucket plug, and the harness-support-matrix `--trace-thread` row. Later steps: bridge v2 `sent` references and the post budget, the control listener, timeline CHAT_REPLY cards, and the toggle UI.

# Acceptance Criteria

- A thread classed by `agentctl_trace` metadata (root or entry) or by the fixed root title never appears in Main, All/flat, the Thread List, unread tallies, or raw-feed search hits; its class is durable in the store and sticky across edits and restarts, and only an owner override changes it.
- The naming path never runs on a transcript thread: no model call, no stored or rendered `slug: description` anywhere, including in the page DOM.
- Transcripts is a top-level view beside Main/All/Thread List in the view picker, listing `agent · day` newest day first with its own scoped search; reading a day shows its chunks in time order with any legacy `— after:` label line dropped. Index and reading are served from the local store/cache; no upstream read starts from a page view.
- Server search returns transcript hits apart from raw hits; the page hides them until the Agent Transcripts bucket chip (shown only while its count is over 0) is on. Bucket defaults follow the owner-146 table as data: on only when searching from the Transcripts screen. Tapping a transcript hit opens the Transcripts view at that message, in context.
- Step 1 adds the `--trace-thread` harvesting row to `common/docs/agentctl/harness-support-matrix.md` in the same commit (claude/codex/muse/headless as built, unknown none).
- Page tests at phone (412x915) and desktop (1280x800) cover the behaviour in light and dark theme, keyboard open (visualViewport shrink) where layout is involved, Discuss half-pane open in at least the phone shots; the standing screenshot matrix (light/dark x keyboard open/closed x phone/desktop, one set per starting screen) goes to the coordinator for liaison review before anything is announced.

# Notes

[watermelon-full-preview] 2026-11-17. Gates (design section 8): (a) #658's bridge half counts as met (bea27327, 0fbb69ba on main); (b) the services turnover exists and ran 2026-11-17. The search bucket row is #1797 (mu-searchscope); step 1 registers the transcripts bucket against the agreed `searchBuckets.register` interface and lands after #1797, or behind a stub of that interface if #1797 slips. Chip wording is a placeholder until the owner confirms it (owner 144). Bare thread-reference rendering in trace posts follows the shared sink (mu-refrender #1796); no fork.

[watermelon-full-preview] 2026-11-18. Step 1 LANDED as 2a585ac2 on main (owner gate pass on b2a05123, verified-tree overlap landing over threads.mjs/threadlist.ts): classification (metadata + title, sticky, additive `thread_class` table outside the migration ladder so older binaries still open the database), the Transcripts view (`agent · day`, scoped search, ET row times), raw-feed exclusion, naming never on transcripts, server-search `transcript_hits` partition, and the Agent Transcripts bucket on the #1797 row (off by default, on from the Transcripts screen; hit tap opens the day at that message). The `--trace-thread` matrix row landed with it. Remaining for later steps: bridge v2 `sent` references and the post budget, the control listener, timeline CHAT_REPLY cards, and the toggle UI. Open question for the owner (handoff only): the main-channel composer still shows in the Transcripts view — hide it there, or keep it?
