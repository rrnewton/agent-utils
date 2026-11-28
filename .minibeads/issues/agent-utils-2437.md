---
title: 'vibe-talk-reply-keyboard-hides-message-top: with the phone keyboard up after Reply, the answered message''s top stands above the screen'
status: closed
priority: 2
issue_type: bug
labels:
- vibe-talk
- owner-ui
created_at: 2026-11-27T22:42:44-05:00
updated_at: 2026-11-28T01:02:00-05:00
closed_at: 2026-11-28T01:02:00-05:00
---

# Description

[opus 5.5, cl-signpost3] Found in the #2428 owner-UI matrix, the same on main and with the fix.

- On a phone (412x915), Reply on a message that has replies: they are gathered under it, and the box stands under them, docked at the foot.
- With the keyboard up (the visual viewport 336 px shorter), the answered message's top stands at -14 px: its header (time, controls) is cut off at the top of the screen.
- With the keyboard down, the same message stands at 132-219 px with the box at 505-733, all on screen.

# Design

- Find where the keyboard's arrival moves the list: the foot dock re-placing the box against the shorter viewport, or the clear-of-box lift (#2395) applied again.
- Keep the answered message's top (its header and first lines) on screen when the keyboard comes up. Lifting the list to clear the box must never push the message's top above the head of the list.
- Nothing moves when the keyboard comes up if the message and box already fit.

# Acceptance Criteria

- At 412x915 by touch: Reply on a message with gathered replies, then the keyboard up. The message's header stays fully on screen, and the box covers neither the message nor its replies' first lines.
- The same for a message without replies (no regression of #2395 / #2063).
- A walk that fails on main; owner-UI evidence, light and dark, with BUILD.txt.

# Notes

Reserved as #2428's follow-up (the coordinator); filed after #2428 landed (e2baf4c3). Its fix follows #2432.

## Resolution

Fixed by 7f578235 (vibe-talk: the phone's keyboard over a Reply keeps the answered message's header on screen). Deployed in 3c5da567; post-deploy guards green: reply_keyboard_head, keyboard_hides_bar, ios_keyboard_send, composer_stays, inline_reply, reply_continuity, reply_box_clear, quote_still, reply_open_instant.

- Cause: keepWrittenBox (#429 keyboard-hides-bar) brought the whole reply box above the keys when the keyboard shrank the list (keepBoxInView). With gathered replies that scrolled 146 px at 412x915 and put the answered message's header above the screen (top -14 px).
- Fix: for a reply in the stream, keepBoxInView never moves the list past the answered message's top (the row marked data-replying). The box comes up as far as that allows, field first. Other callers are unchanged.
- Walk: tests/reply_keyboard_head_browser.py (dag 193). Main had the header at -3 px.
- Accepted trade-off (liaison 1125): on a 393x852 phone, gathered replies may sit under the docked box while the message and the field stay visible; at 412x915 the box's button row may sit under the keys while Send shows. Revisit together with #2438 if the owner picks docking there.
