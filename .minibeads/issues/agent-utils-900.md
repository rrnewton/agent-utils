---
title: 'agentterm-paste-escape-replies: a paste into an agentterm pane carries no accumulated terminal query replies'
status: open
priority: 2
issue_type: bug
assignee: cl-start
created_at: 2026-11-05T17:36:49.214870009+00:00
updated_at: 2026-11-05T17:36:49.214870009+00:00
---

# Description

Every paste into an agentterm pane target carries about 2 KB of terminal escape replies ahead of the real message. On 2026-11-05 an owner chat message pasted through the interim agentterm target arrived in the session as input that began with repeated OSC 10/11 colour replies:

    ]10;rgb:dcdc/dcdc/dcdc\]11;rgb:0000/0000/0000\]10;rgb:dcdc/dcdc/dcdc\ ...

The agentterm client queries the terminal's foreground and background colours. Herdr answers into the pane's input line, the replies accumulate there, and the next paste submits them along with the message. The message itself arrives intact. But every delivery to an agentterm target wastes context, and the owner wants agentterm to be a first-class target.

# Design

- In agentctl's pane adapter, before a paste into an agentterm target: drain or strip pending terminal query replies from the pane's input line. That covers OSC 10/11/4 colour replies, cursor-position reports (CSI ... R), device-attribute replies and similar responses. Then paste.
- Prefer a clean input line verified by reading it back. Where the adapter cannot clear it from outside without risking the agent's own draft, refuse to guess. In that case, work with agentterm's owner so the client consumes its own query replies, or stops issuing the colour queries, rather than echoing them into input.
- The cleanup must never delete text a human typed. Strip only well-formed escape replies.

# Acceptance Criteria

- Test: a pane whose input line holds accumulated OSC 10/11 replies receives a paste. The submitted input is exactly the message, with no escape replies.
- Test: a pane whose input line holds human-typed text plus replies. The typed text is preserved (or the paste is refused as now); only the replies go.
- Live check: one message to an agentterm target, then its session history shows input that begins with the message header.

# Notes

Found by the coordinator on 2026-11-05 at 12:36 PM ET while confirming the #896 interim route end to end. The lead set it to P2 (the owner wants agentterm first-class). Assigned to cl-start with the #852 agentterm work, after #896 and #897.
