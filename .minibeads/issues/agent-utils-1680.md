---
title: 'inline-reply-keyboard-clutter: two Sends overlap and chips cover the reply form while typing'
status: open
priority: 1
issue_type: bug
created_at: 2026-11-15T19:58:00.000000000+00:00
updated_at: 2026-11-15T19:58:00.000000000+00:00
---

# Description

Owner UI bug 102 (2026-11-15, Android phone, Gboard): "Do you see how the
send button is overlapping the underneath send button? That's pretty bad.
Did we add a second button for iOS? What are some options for improving
this. It seems when we pop the keyboard for this screen we need to
simplify. We've got these additional chips including the undo chip getting
in the way."

The screenshot shows the inline reply form ("Your reply to" a bot) with its
own row (Post to main, Cancel, Send reply); that Send is half covered by a
second floating "Send" at bottom right, and the chips "10 new (10)",
"Bookmark", "Unread" and "Undo" float over the form's buttons.

Found on github/main:

- The second Send is #keyboard-send (vibe-talk/web/voice.html), the Send
  at the keyboard's top from #1504 ios-keyboard-send (commit 6be8d8d0,
  owner request 048 on iOS: on iOS the keyboard showed no Send and the
  page Send hid under it). Yes, it was added for iOS; the rule is
  browser-generic (the visual viewport), so Android gets it too.
- It stands from renderCornerSend (vibe-talk/web/src/view/composer/
  keyboard.ts) while the focused box's own Send is not fully inside the
  visual viewport. The reply in the stream (reply-text) is a corner-Send
  field (KEYBOARD_SEND_PARTS) but is never fitted (KEYBOARD_FIT_PARTS
  holds only the channel's and the side column's boxes), so its row can
  straddle the keyboard's top, half on screen: a fully-inside test then
  stands the corner Send over the visible half. Two Sends, overlapping.
- A second race: the browser's own reveal scroll, which shows the
  focused box as the keyboard comes up, moves the row's Send after the
  measure that stood the corner Send, and nothing re-said which Send
  stands on that scroll.
- The chips are #scroll-tools (jump to the newest, to the bookmark, to
  the unread, to the reading, Draft, Back to reply, and the Undo chips),
  absolutely positioned at the list's foot. Nothing stands them down
  while the keyboard is up, so with the layout shrunk they land on the
  reply form's buttons.

# Design

Options 1-3 built together, as the owner was offered:

1. ONE Send only. renderCornerSend stands the corner Send only while no
   part of the focused field's own Send is in the visual viewport (an
   intersection test, not fully-inside), and settleOnScroll says again
   which Send stands after the browser's reveal scroll. The focused
   field's own row is the one Send; the fitted composers (channel, side)
   keep the corner Send, their row resting under the keys by #1135.
2. No floating chips while the keyboard is up. In the coarse-pointer
   data-keyboard=open regime (vibe-talk/web/styles/voice/
   19-control-pane.css): the jump chips (newest, bookmark, unread,
   reading), Draft and Back to reply stand down; the chips row moves to
   the top of the list, where the three Undo chips (a dismissal, a
   dismissed send, a removed bookmark) stay reachable. The chips' own
   hidden state is untouched, so the whole row stands again at the
   list's foot, exactly as drawn, when the keyboard goes down or the
   field loses the focus. Undo offers keep their own timers either way.
3. Compact reply row while typing. Same regime for #reply-compose: the
   helper text (the destination line, and who is being answered beside
   the label) goes, an empty state line takes no room, and the actions
   row is one non-wrapping line: Post to main a small toggle at its
   start, Cancel and Send reply at its end. All of it returns with the
   keyboard down.

No change to vibe-talk/web/voice.js or web/src/app/main.js. Option 4
(Send as an icon inside the text box) is not in scope unless asked.

# Acceptance Criteria

- Module tests (vibe-talk/tests/js/view/composer.test.mjs) pin the
  corner Send: a reply Send half on screen stands it down, wholly under
  the keyboard stands it up pressing the reply's own Send, and the
  reveal scroll stands it down with no new keyboard measure.
- A browser test (vibe-talk/tests/reply_keyboard_clutter_browser.py) at
  393x852 by touch with the on-screen keyboard simulated by a shorter
  viewport, as #610's tests do, checks the inline reply form and the
  main composer: exactly one Send standing, no chip display over the
  form, the reply row compact on one line, and chips and helper text
  back when the keyboard goes down. At 1280x800 with a mouse: no corner
  Send, the chips keep the list's foot (desktop unchanged).
- Screenshots of the changed states are looked at, not only asserted.

# Notes

Owner UI fast-track (owner request 102). References #1504
ios-keyboard-send, #575/#610 mobile-keyboard-compose, #1135
raw-message-page, #429 keyboard-hides-bar.
