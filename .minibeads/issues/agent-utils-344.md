---
title: 'search-shortcut: Cmd+/ (Ctrl+/) opens the search from anywhere, and Help moves to Cmd+?'
status: open
priority: 2
issue_type: feature
labels:
- vibe-talk
created_at: 2026-10-18T19:24:55.946481919+00:00
updated_at: 2026-10-18T19:24:55.946481919+00:00
---

# Description

[opus 5.5] Owner, 2026-10-18: "want reliable cmd-/ for search ... Sometimes just pressing '/' doesn't work ... prefer it to just be Cmd-/. Currently that pops open help ... map it to Cmd-? (i.e. Cmd-shift-/)."

Today "/" opens the search only when the focus is not in a text field, not on a picker or slider, and only while single-key shortcuts are on. After sending a message the focus stays in the message box, so "/" types a slash. After choosing a channel or view with the mouse the focus stays on the picker, so "/" goes to the picker and nothing visible happens. Ctrl+/ (Cmd+/ on a Mac) opens the list of keys instead.

Wanted:
- Cmd+/ on a Mac and Ctrl+/ elsewhere ALWAYS opens the search with its field focused, whatever holds the focus: a message box with a draft (the draft is untouched and no slash is typed), any other text field, a picker, a slider, a button, a selected message or card, the thread view, the right column, Settings, Help, an open menu or the reading-pace popover (closed first), the reply screen (left with its draft kept). It works with single-key shortcuts turned off. A held chord opens the search once. Nothing happens on the sign-in screen, where there is nothing to search.
- The chord is heard before any other listener on the page, so nothing on the page can swallow it, and the browser's own default for it is refused.
- Cmd+? (Cmd+Shift+/) on a Mac and Ctrl+? elsewhere opens the Keyboard entry of Help, as Ctrl+/ does today. "?" alone and the Help link still do, and stay the way in if a Mac menu takes the chord first.
- Plain "/" keeps working wherever it works today, and still types a slash in a text field.
- On layouts where "/" needs Shift (for example Shift+7), Cmd+/ means that key with Cmd. A chord typed with AltGr is never the search. Nothing fires while an input method is composing.
- The list of keys, the search button's tooltip, aria-keyshortcuts, the Settings hint and the README say the new chords, and say that Ctrl+/ no longer opens the list of keys.
- Page tests, and a Chromium check pressing real keys through CDP: with the focus in the message box, with the pace popover open, with a menu open, on Settings, on Help, in the right column, on the Reply screen (draft kept), on an emulated Mac, with a Shift-for-slash layout, and with a listener that stops propagation.
