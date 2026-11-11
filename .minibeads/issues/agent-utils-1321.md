---
title: 'after-done-setting: a synced dropdown chooses what e does after marking Done (Stay, Next message, Next unread)'
status: closed
priority: 0
issue_type: feature
labels:
- vibe-talk
- owner-ui
created_at: 2026-11-11T16:36:22+00:00
updated_at: 2026-11-11T16:36:22+00:00
---

# Description

[opus 5.5] Owner request (reply 391): "Actually on second thought I would actually like the e-jumps-to-next-unread as an optional toggle in settings, default off. Once I have the shift arrows I don't know if I like it jumping. In fact I would like a drop down for that key either jumping to next message, jumping to next unread, or nothing."

Today e and Mod+E (`cmd-e-done` in web/src/view/shell/keymap.ts, `doneFromKeyboard`) mark the selected message Done and move on to the next unread below.

# Design

- A synced setting with a dropdown labelled "After Done (e)": Stay (the default), Next message, Next unread (today's behaviour).
- It applies to e and Mod+E. Marking is unchanged; Shift+E (#295 shift-e-read-collapse) is unchanged.
- Key hints, the key's help text (the `does` text) and any help page follow the chosen setting.
- Built beside #1314 unread-step-keys (Shift+arrows, same file) but landed separately.

# Acceptance Criteria

- Default Stay: e marks Done and the selection stays on the message.
- Next message: e marks Done and selects the next message below, read or not.
- Next unread: e marks Done and selects the next unread message below (today's behaviour).
- The setting syncs like the other synced settings and survives a reload.
- One browser test per choice, plus a check that the help text follows the setting.
- Owner tier, path-selected gate, deployed on landing.

# Notes

Related: #1314 unread-step-keys, #295 shift-e-read-collapse. The owner has been told Stay is the new default.

## Resolution

[watermelon-full-preview] Delivered: a synced setting `vibe-talk.voice.after-done` (server
`SYNCED_KEYS` and the page's synced list) shown as a dropdown "After Done (e)" in Settings'
Messages section — Stay (the default, nothing kept), Next message, Next unread. e and Mod+E
read it at each press: Stay lands back on the marked message, Next message selects the row
below read or not, Next unread keeps the previous behaviour; putting a message back never
moves, marking is unchanged, Shift+E unchanged, and landing still goes through the single
scroll owner's path. The keymap's `does` text, the key hints and the Help list word e by the
choice and never promise the next unread under Stay. Covered by module tests for all three
landings and the help wording, and by a 1280x800 browser walk (one pass per choice through
the Settings dropdown, plus the help text under Next unread and under Stay). Settings page
looked at on desktop 1280x800 and on a phone viewport.
