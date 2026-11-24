---
title: 'agentctl-chat-headless-midturn-replies-lost: mid-turn reply blocks after the first are silently dropped'
status: open
priority: 1
issue_type: bug
labels:
- agentctl
created_at: 2026-11-24T07:31:46-05:00
updated_at: 2026-11-24T07:31:46-05:00
---

# Description

[watermelon-full-preview] On the agentctl Slack chat bridge for a headless target, the agent printed five well-formed reply blocks in one long turn. Only the first posted. Two blocks sat together in one assistant message; two others each stood alone in their own message, followed by a tool call. `agentctl chat sent N` showed "holds no replies" for all four lost blocks about 15 minutes later; `agentctl chat reply N --file F` then sent all four. Meanwhile `chat health` reported OK ("last reply sent" from the first reply). Prior work #1974 agentctl-headless-chat-transcript-parity (closed) made headless targets follow the headless event log as the transcript for CHAT_REPLY capture.

Reproduced with a synthetic headless event log (one request, one turn): blocks numbered with sequential reply IDs (the assigned ID, then +1 for each later reply, the incrementing pattern the bridge itself teaches for nonce IDs) are held by the transcript reader but capture stores only the first. The same log with every block under the prompt's own ID stores all five, so message grouping, a following tool call, turn-end timing and body dedupe are not the drop.

Root cause: reply-ID recognition is exact-match only (`recognized_reply_marker` in `rs/agentctl/src/chat_runtime.rs` accepts only the alias assigned to that request, or `<nonce>_<ordinal>` for its nonce). Well-formed blocks under sequential unassigned IDs belong to no request: `merge_transcript_blocks` filters them out before storage, the headless screen is empty so nothing reports them as unknown, and the capture-miss watch never sees them because `find_misses` only checks unanswered candidates (a request with a stored reply is excluded) for that candidate's current reply ID. Once reply one lands, blocks two to five are invisible to capture, to `chat sent` and to health.

# Acceptance Criteria

- Every well-formed block a headless target prints mid-turn is captured and posted once, in order, with no duplicates: several blocks in one turn, two in one assistant message, one followed by a tool call, and one at the end of the turn, including sequential reply IDs that continue an open request's assigned ID and are assigned to no other request. Both headless (event log) and TUI (screen plus transcript) targets.
- `chat health` (and/or the capture-miss watch) alarms when an admitted request has a printed but unsent block older than a bounded grace, including when an earlier reply to the same request was already sent.
- Deterministic regression tests for the exact defect fail before the fix and pass after it, in both editions (Python and Rust) where the tool has both. Condition-based waits only.
- Only test channels or fakes: never a real channel, never the owner's identity, never the live voice server.

# Notes

[watermelon-full-preview] Pre-assigned #2205 agentctl-chat-headless-midturn-replies-lost, priority P1: owner-facing replies are being lost. Root cause reported to the coordinator before the fix. No new persisted bridge-state fields are planned; any change there is announced before gating.

[watermelon-full-preview] Prompt/capture agreement (coordinator review): the prompt teaches reuse — the same reply ID for every reply to a request with a short ID, and incrementing only for a numbered `<nonce>_<ordinal>` ID (the chat skill already taught exactly this). That teaching stays; the bridge prompt footer now says "the same reply ID for every reply to this request" in words, capture accepts a sequential short-ID run in the transcript as a fallback (ordered, consecutive, run started, assigned to no other request), and anything else is alarmed, never guessed. Screen reads keep the older rule: a sequential number on screen matches no request and is reported, so a quoted or stale block is never sent.
