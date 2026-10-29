---
title: 'chat-bridge-pane-session-identity: verify the target pane''s session before typing'
status: open
priority: 1
issue_type: bug
created_at: 2026-10-29T12:31:44.241371459+00:00
updated_at: 2026-10-29T12:31:44.241371459+00:00
---

# Description

[watermelon-full-preview] Owner request 2026-10-29 (chat-bridge-robustness batch).

Requests were typed into the wrong pane: an idle older agent's pane, not the registered target. Two panes of the same harness, in the same directory, under the same tab label are indistinguishable by label, kind and directory; only the agent session identity tells them apart, and a record whose session was asserted by an operator did not have that session checked before typing (an asserted session also never satisfied the recipient check, so such a record without a pinned harness process could never be typed into at all).

Wanted: before typing, verify the target pane's agent session identity, not just its tab label. A pane reporting another session, or none, is refused; a pane still reporting the asserted session satisfies the recipient check.
