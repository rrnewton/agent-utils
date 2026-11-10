---
title: 'raw-message-page: scrollable main-channel composer with a Send raw message checkbox'
status: closed
priority: 1
issue_type: feature
assignee: mu-raw
created_at: 2026-11-09T14:01:57.596871127+00:00
updated_at: 2026-11-10T07:02:41.223897381+00:00
closed_at: 2026-11-10T07:02:41.223897261+00:00
claimed_at: 2026-11-09T14:02:35.611561613+00:00
claimed_until: 2026-11-11T14:02:35.611468413+00:00
---

# Description

Owner request (reply 240, 2026-11-09): on the main channel in vibe-talk on Android, the composer cannot be scrolled past the message box. The rigid stop was deliberate (sending from the keyboard makes the Send button redundant), but the owner wants the default resting scroll to keep the box directly on top of the keyboard while still being able to scroll further down to reveal the Send and Cancel row. That same row gains a Send raw message checkbox.

Without this, the Send and Cancel controls are unreachable with the keyboard open, and there is no way from the phone to send text to the bridged agent exactly as typed (no header, no footer), so a slash command typed on the phone cannot run as a slash command.

# Design

Page-only change to the main-channel composer in vibe-talk (Android first).

- Remove the rigid scroll stop at the message box. The default resting scroll keeps the box directly on top of the keyboard, but the user can scroll further down to reveal the Send and Cancel row.
- In that same row: a Send raw message checkbox, unchecked by default, reset after each send, with the label and aria exactly as written and reachable by keyboard.
- Keep everything else: the keyboard-open behaviour, safe areas, the reply-231 bottom bar, and the reply composer's Post-to-main checkbox.
- When the box is checked, vibe-talk marks the post as raw for the bridge (see the companion bridge issue) and shows the text as typed plus a small raw marker. When unchecked, posting is unchanged.
- Prefer the page modules. Any need to touch the legacy voice script is coordinated first, since another lane is moving its pieces.

Coordinator decision (2026-11-09): the revealed row is Send + Cancel + Send raw message. The main composer has no Cancel today, so add one: Cancel clears the draft and drops the keyboard, matching the reply composer's Cancel semantics (with words in the box it asks first, as there). Mirror the reply composer's row styling. The one-line rule for slash commands (a slash command is sent as its own one-line raw message) goes in the page help.

# Acceptance Criteria

- At 360 px and 412 px phone widths with the keyboard open, the message box rests directly on top of the keyboard by default, and scrolling further down reveals the Send and Cancel row with the Send raw message checkbox in that row.
- The checkbox starts unchecked, resets to unchecked after each send, is reachable by keyboard, and exposes the label Send raw message to assistive technology.
- With the box unchecked, sending behaves exactly as before (including the keyboard-open behaviour, safe areas, the bottom bar, and the reply composer's Post-to-main checkbox).
- With the box checked, the post is marked raw and the message shows the text as typed plus a small raw marker.
- Page tests cover the scroll stop, the revealed row, and the checkbox; a real-browser check covers both phone widths with the keyboard open.

- The revealed row holds Cancel beside Send and the Send raw message checkbox; Cancel with words in the box asks first, then clears the draft and drops the keyboard; the page help states that a slash command is sent as its own one-line raw message.

# Notes

Owner request reply 240 (2026-11-09). Companion: #1136 raw-message-bridge. Slot raw, branch mu-raw/raw-message. Page home: vibe-talk page modules; coordinate before touching the legacy voice script (another lane is moving its pieces for #624).

Renumbered (2026-11-09): filed first under a number another lane landed on main meanwhile; this number is the one on main.
