---
title: 'agentterm-paste-escape-replies: a paste into an agentterm pane carries no accumulated terminal query replies'
status: closed
priority: 2
issue_type: bug
assignee: cl-start
created_at: 2026-11-05T17:36:49.214870009+00:00
updated_at: 2026-11-06T14:45:00.000000000+00:00
closed_at: 2026-11-06T14:45:00.000000000+00:00
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

## Progress (cl-start, 2026-11-05)

[opus 5.5] Cause, from a probe and agentterm's source:
- Herdr is not holding anything back. A raw-mode probe in a Herdr pane asked for the OSC 10/11 colours every 5 s, and each answer came back within the same 10 ms as one 50-byte `ESC]10;rgb:dcdc/dcdc/dcdc ESC\ ESC]11;rgb:0000/0000/0000 ESC\`.
- agentterm re-asks every 5 s while focused, and on focus-in and resize. It drops an answer only while it still awaits one: a 3-second window, `REPLY_WINDOW` in its `src/render/background.rs`.
- An answer read later goes to key handling: `ESC ]` arrives as Alt+`]`, the body as characters, `ESC \` as Alt+`\`. The composer fills with exactly the text the session received: `]10;rgb:dcdc/dcdc/dcdc\]11;…\`.
- Herdr's paste is clean. The replies were already sitting in the composer, and the paste plus Enter submitted them together.
- The root fix is agentterm's: drop late colour answers, or stop treating them as typed text. That needs a report to agentterm's owners.

agentctl change:
- Before pasting into an idle agentterm composer, agentctl checks whether it holds nothing but terminal replies: OSC 10/11/12/4 colour replies, cursor-position reports, status reports and device attributes, with the row breaks of a wrapped composer ignored.
- If so, it presses Ctrl+C, agentterm's documented "clear the input line", and the composer must read back empty within 3 s before the paste. If it does not, the delivery is refused and nothing is typed.
- Any other text beside the replies is a person's draft: nothing is cleared, and the delivery is refused as before.
- Tests: accumulated replies are cleared and the message is submitted alone; typed text beside replies is kept and the paste refused; a clear that does not take refuses the paste.

Remaining: the live check (one message to an agentterm target; its session history must begin with the message header). That goes through the lead.

## Resolution

Landed in de02b9f3's stack: an idle agentterm composer holding only leaked terminal query replies is cleared (Ctrl+C, read back empty) before the paste; anything else is still refused. Gate: the full gate passed on the pre-rebase stack (f1226d35 on a7087281). The rebase onto ab1d7b16 overlapped agent_watch.rs and failed_digest.rs; targeted agent_watch, failed_digest and revive tests (202) and clippy passed on the rebased tree.
