---
title: 'ios-keyboard-send: on iOS the keyboard shows no Send and the page Send hides under it'
status: closed
priority: 1
issue_type: bug
created_at: 2026-11-13T23:19:00.000000000+00:00
updated_at: 2026-11-13T23:19:00.000000000+00:00
---

# Description

Owner request 048 (2026-11-13): on iOS the keyboard does not include a Send
button at all, and the page's Send sits below the box being written in, often
off screen or occluded by the keyboard, so sending means first shrinking the
keyboard to go and find Send. Having Send on the keyboard is the idiomatic
approach. Follows #575/#610 mobile-keyboard-compose, which made a phone's
return key send and added a corner new-line key.

Found on github/main:

- Every sending box is a multiline textarea. `renderEnterSends`
  (vibe-talk/web/src/view/composer/boxes.ts) sets `enterkeyhint="send"` only
  on `COMPOSER_FIELDS`, and only while Enter to send is on. The quick reply
  (`quick-reply-text`) and the Discuss follow-up (`discuss-followup`) both
  send on Enter through the same key rule but were missing from
  `COMPOSER_FIELDS`, so their return keys were never named and `isComposer`
  never read them as composers.
- Enter to send itself is per device and ON unless turned off there
  (`onUnlessOff`, absent reads as on; it is not synced across devices). The
  iOS default is therefore already on; an explicit off on that device is kept
  as the owner set it. The missing Send is not the setting.
- Even with the setting on, WebKit draws a plain return, not Send, in a
  multiline box on some phones: `enterkeyhint` names the key the keyboard
  should draw, and that drawing is the keyboard's. Enter still sends through
  the key rule. No `inputmode` was stated beside the hint.
- The row's Send stands below the box by design (#1135): with the keyboard up
  the box rests on the keys and the row is a scroll away, which is exactly the
  hunt the owner reports.

# Design

- `COMPOSER_FIELDS` (vibe-talk/web/src/view/composer/keys.ts) names every box
  whose words a Send posts: the call's, the channel's, its right-column twin,
  the reply in the stream, the floating quick reply and the Discuss follow-up.
- `renderEnterSends` names each one's return key for the setting as it stands
  (`enterkeyhint` send/enter) and keeps a text keyboard beside it
  (`inputmode="text"`), never undoing the `inputmode="none"` dictation holds
  the keyboard down with. The shell says `enterkeyhint="send"` as served, the
  on-by-default reading, so the first focus agrees before the saying runs.
- A Send stands at the keyboard's top (vibe-talk/web/src/view/composer/
  keyboard.ts, `#keyboard-send`): while the keyboard is up over a sending box
  whose own Send stands outside the visual viewport, it stands in the corner
  like the new-line key, its foot counted from the visual viewport
  (`offsetTop + height`) on every measure the viewport's resize and scroll
  already ask for, and pressing it presses that box's own Send. While the
  row's Send already stands in the visible part it stands down, so the two
  never stand together; no keyboard, and it is down.

# Acceptance Criteria

- Module tests pin `COMPOSER_FIELDS` coverage (every sending box, the pace
  box excluded), the `enterkeyhint`/`inputmode` pair per setting in every
  box with dictation's held-down keyboard left alone, and the corner Send:
  shown at the keyboard's top only while the row's Send is outside the
  visible part, pressing that very Send, and down with the keyboard.
- WebKit (Playwright webkit) at an iPhone size checks the attributes, a real
  Enter sending from the reply box, and the corner Send's place under a
  simulated visual-viewport shrink
  (vibe-talk/tests/ios_keyboard_send_browser.py).

# Notes

Owner fast-track, page-only. References #575/#610 mobile-keyboard-compose.

## Resolution

Landed on main. What a real iPhone alone can say, said plainly: the label
the phone's own keyboard draws for `enterkeyhint="send"` in a multiline box,
and how the real keyboard and Safari's pan move the visual viewport as it
comes up. The WebKit check simulates the shrink on the page's own viewport
numbers; where this change was verified, the WebKit browser itself could not
be fetched (the proxy refused the download) and the same check was run in
Chromium at the iPhone size instead, with the WebKit run left for CI. Enter
sending, the attributes and the corner Send's place do not depend on that
drawing: Enter sends through the key rule in every sending box, and the
corner Send stands above where the keyboard will be whether or not the
keyboard draws its key as Send.

Follow-up (registration): the browser check is registered as
`check.ios-keyboard-send-browser` in vibe-talk/tests.dag.yaml with a
`make ios-keyboard-send-browser` target — Chromium at iPhone size by
default, `--browser webkit` where that browser is installed, and a missing
WebKit is a skip with its reason, never a failure.

Process note: while building the combined tree after the gate (main had
moved under the gated commit), one read-pause dictation assertion was
changed (a kept-down keyboard reads as any input mode but `none`, the page
now stating `text`) and a second was dropped where #1503's restart-on-end
landed over it. That post-gate test edit was covered by recorded
combined-tree checks (both typecheck configs, parse, the touched module
tests and the read-pause browser walk), not by a fresh gate of that exact
commit; next time a post-gate test edit gets a re-gate.
