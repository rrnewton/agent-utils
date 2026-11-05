---
title: 'chat-notsent-after-posted: never tell an agent a reply was not sent once the bridge posted it'
status: closed
priority: 1
issue_type: bug
assignee: cl-start
created_at: 2026-11-05T15:27:40.515166915+00:00
updated_at: 2026-11-05T16:06:58.000000000+00:00
---

# Description

P1. The chat bridge types "Chat reply not sent: the screen showed no closing line for the reply block marked N" into a coordinator's pane for a reply block that has already been posted. The agent then resends, and the owner sees duplicate replies.

Seen twice on 2026-11-05 on a consumer host's bridge, agentctl d8447c04:
- **Request A:** reply 1 posted at 12:43:19Z; the not-sent notice came; the agent's resend posted reply 2 at 12:45:37Z.
- **Request B:** reply 1 posted at 15:10:11Z; the notice came; the resend posted reply 2 at 15:22:52Z.

`agentctl chat sent` shows both replies for each request.

# Design

- Before typing any not-sent notice, check the bridge's own sent journal for that request. If a reply for the request was posted since the block began, type nothing. Alternatively, name the posted reply and say explicitly not to resend.
- Find why fence capture judged "no closing line" when the reply had been captured and posted. A likely suspect is a double-capture path: the transcript path posts the reply, while the screen path sees a cut-off fence and raises the notice. One capture of a block must settle the block for every path.

# Acceptance Criteria

- Test: a reply posted through the transcript path, or through `chat reply`, while the screen shows a cut-off fence for the same block, produces no not-sent notice.
- Test: a genuinely unposted, unclosed block still produces the notice.
- Neither of the two reported sequences can recur (covered by the tests above).

# Notes

Reported by the lead on 2026-11-05 at 10:26 AM ET, from a consumer team (P1; the owner noticed duplicate replies). Assigned to cl-start.

## Resolution

[opus 5.5] cl-start, 2026-11-05.

**Sequence** (the consumer host's bridge log and its coordinator queue, read-only), the same all three times:

| Reply ID | Transcript captured and sent reply 1 | Not-sent notice queued (`queued_at`) |
|---|---|---|
| 5826 | 12:43:18Z | 12:45:23Z |
| 5829 | 15:10:09Z | 15:14:00Z |
| 5835 | 15:23:22Z | 15:26:40Z |

- **What happened:** minutes after each send, a recovery scan of the pane read that block's opening line and first rows, without its closing line, as a block with no closing line.
- **Why the existing check missed it:** the check that silences the start of a stored reply compared the pane's text with the stored text, ignoring only whitespace and box drawing.
  - The stored text is in its own formatting: `*bold*`, `•`/`◦` bullets, `[label](url)` links.
  - The pane draws it rendered, so the two never matched.
- **Not a race:** each notice was queued after its reply was stored.

**Fix:**
- **One capture settles the block for every path.** A block seen only in part (no closing line, or unopened) under a reply ID whose reply is already stored is never reported:
  - under a short alias, which names the request, once the request has any stored reply;
  - under a long `<nonce>_<ordinal>` ID, which names one reply, once that reply is.
- **The race is closed too.** A not-sent notice still waiting in the coordinator's queue is taken back out, untyped, once every partial block it reports is under such an ID (`withdraw_answered_feedback`, checked each pass). Its entries are recorded as reported. The service logs the withdrawal.
- **Blocks with a misplaced or miswritten tag** are still weighed against the stored replies' text, as before.
- **Trade-off:** a genuinely cut-off second block under a short ID whose request already has a reply is no longer reported. The userguide says so and points at `chat reply`.

**Tests:**
- `a_reply_sent_from_the_transcript_is_never_reported_unsent_from_its_cut_off_rendering` covers both the transcript path and `chat reply`, with markup the pane renders.
- `an_unsent_unclosed_block_is_still_reported_and_its_notice_withdrawn_once_it_is_sent` checks two things:
  - a genuinely unsent block is still reported;
  - its queued notice is withdrawn once the reply is stored, and the block is not reported again.
- Four existing tests changed where they expected a report or log for a partial block under an already-answered ID.
