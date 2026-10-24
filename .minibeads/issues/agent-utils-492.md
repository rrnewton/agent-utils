---
title: 'threads-row-widths: a thread unpacked in the Threads view reaches its card''s right edge, the owner''s rows the narrower, and its reply box as wide as its rows'
status: closed
priority: 2
issue_type: bug
labels:
- vibe-talk
created_at: 2026-10-24T11:45:20.672374669+00:00
updated_at: 2026-10-24T13:02:35.820666288+00:00
closed_at: 2026-10-24T13:02:35.820666157+00:00
---

# Description

[opus 5.5] The owner (rescued Slack message 1792676389, 2026-10-22, Threads view on a desk, inline reply open): "The thread messages are less wide than the card above them AND the 1st message in the thread. I understand that they need to be indented on their left edge, good. But why does the right edge need to come in!!? ... that same size that I want used for the messages in the thread, i would like the reply box to match that so it is very clear that we are replying in the thread ... right now the messages from ME are wider and the responses are shorter ... I want it flipped. My messages should be the shorter ones. And I think the difference in their width could be HALF what it is now." Measured on main 285c0885 in Chromium at 1280x800: card 274-1006 px, the owner's rows 366-1006, the agent's 366-910, other people's 366-958, the reply box 274-1006; at 393x852 the agent's rows overrun the card to the window's edge.

# Acceptance Criteria

In the Threads view every row of an unpacked thread ends at the card's right edge; each starts at the thread's indent, the owner's half the old difference further in (min(15%, 6rem) / 2); a reply opened under one of them spans the thread rows' width; the bridge's spine stays one line. Page tests on the rules; Chromium at 1280x800 and 393x852.
