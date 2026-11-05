---
title: 'chat-notsent-after-posted: never tell an agent a reply was not sent once the bridge posted it'
status: open
priority: 1
issue_type: bug
assignee: cl-start
created_at: 2026-11-05T15:27:40.515166915+00:00
updated_at: 2026-11-05T15:27:40.515166915+00:00
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
