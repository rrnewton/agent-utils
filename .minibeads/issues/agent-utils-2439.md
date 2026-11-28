---
title: 'chat-never-truncate: split long chat replies and inbound texts instead of refusing or cutting them, and re-split a post Slack refuses as too long'
status: open
priority: 0
issue_type: bug
assignee: cl-bridgeretire
labels:
- agentctl
- chat
- slack
created_at: 2026-11-27T23:12:00-05:00
updated_at: 2026-11-27T23:12:00-05:00
---

# Description

Step (c) of #2420 relay-never-lossy. The owner decided on 2026-11-27: no lossy transport; a check may warn but never drop or delay a message. The liaison ruled on the size limits ("deliver, tag, count"). The #2420 audit found three chat-path size limits that refuse or cut a message:

- **C5, chat run (agentctl):** a reply over 30,000 bytes is refused, and so is a reply past the per-request byte cap. Code: `chat_runtime.rs`, the reply byte checks (audit lines 5469, 18367-18369, 21555, 14756-14760).
- **S4, the Slack plugin:** an inbound owner text over 32,000 bytes reaches the bridge with its tail cut (`chat-subscription-slack/src/plugin.rs` around 1521-1529).
- **S8, the Slack outbound helper:** a post Slack refuses permanently (`msg_too_long`, `invalid_blocks`) is given up (`chat-subscription-slack/src/cli.rs` around 928-935 and 972-973).

# Design

- C5: a reply over the byte cap is split into consecutive replies marked `(part k/N)`, never refused. The same applies to the per-request byte cap. Splits fall on line ends where possible, never inside a code fence's opening line.
- S4: the plugin passes the full inbound text up to 512 KiB (the frame limit is 1 MiB). Only above that is the text cut, with a one-line `[truncated: N of M bytes]` marker. The plugin and the bridge are built together, so the bound changes on both sides at once.
- S8: on a permanent too-long or invalid-blocks refusal, the helper re-splits the text smaller (halving, down to 2 KB parts) and retries. It never gives up while a smaller split is possible. A failure at 2 KB is reported loudly, never dropped silently.
- Warnings stay one short plain line (lead-014).

# Acceptance Criteria

- C5: a test sends a reply over 30,000 bytes and sees N replies `(part k/N)` posted, together carrying the whole text. It fails on main.
- S4: a test feeds an inbound text of 100 KB and sees the bridge receive all of it. It fails on main.
- S8: a test whose fake Slack refuses posts over a size with `msg_too_long` sees the text re-split and posted whole. It fails on main.

# Notes

- Split from #2420 (audit rows C5, S4, S8) on 2026-11-27, numbered by coordb. Relay size handling (R3, R12) stays under #2420. Request-store and gap handling are #2440.
