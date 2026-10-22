---
title: 'back-key-consistency: Ctrl/Cmd+[ is Back and Ctrl/Cmd+] is Forward in every view'
status: open
priority: 1
issue_type: feature
labels:
- vibe-talk
created_at: 2026-10-22T13:54:12.612138147+00:00
updated_at: 2026-10-22T13:54:12.612138147+00:00
---

# Description

[opus 5.5] The owner, 2026-10-22: "if I press Cmd-[ on the thread list view it jumps back to the Main view but it does NOT have kind of navigation (or back?) effect on other views." Cause: the page binds no Ctrl/Cmd+[. On a Mac Cmd+[ is the browser's Back, and #304 back-gesture-in-app gives a history entry to what is open over a view (the thread list, a thread, the right column's thread, a reply, Settings, Help, a menu) but deliberately none to a view switch (Main, All, two columns), so the browser's Back went somewhere only where one of those was open: the thread list went back to the view it was chosen from (#281 view-keys, threadListBack).

# Design

THE RULE. Ctrl/Cmd+[ goes Back and Ctrl/Cmd+] goes Forward, from anywhere, in every view and screen. The page keeps a history of the PLACES the reader has been on the channel since it was chosen: a place is the screen (the channel, Settings, Help) and, on the channel, the view (Main, All, Thread list, Two columns) with the thread open in it (across the window, or on the right in two columns). Every move to a new place, by any control (the view keys and picker, the layout button, a thread entered or left, Settings, Help and their Back, the browser's Back), is added after the place the reader is at, and any places ahead of it are dropped, as a browser does. Back goes to the place before, Forward to the place after; neither adds one. A place that cannot be shown any more (a thread no longer known, two columns in a window too narrow for them) is skipped. With a menu or a reply open, Back first closes it, as Esc and the browser's Back do (the reply's words kept as a draft); Forward first closes a menu. At either end the key says there is nowhere further and does nothing: it never leaves the app. Choosing another channel starts its history afresh. The call view is not a place: there the keys say so and do nothing.\n\nTHE SPECIAL CASES. #304's closes are this rule where the place before is the one under what is open: a thread entered from the thread list goes back to the list, the list back to the view it was chosen from (whatever chose it), the right column's thread back to its cards, Settings and Help back to where they were opened. So the thread list's Back is no exception; Main, All and two columns simply gain theirs.\n\nWHAT STAYS. The browser's own Back (its button, Alt+Left, a mouse button) and the phone's back gesture keep #304's meaning, closing what is open and then leaving the app: a phone gesture that walked back through every view switch would keep a reader in the app. The page takes Ctrl/Cmd+[ and ] from the browser (heard first at the window, as Ctrl+S is, #403), so the keys follow the whole history; the bare [ and ] still move between two columns (#229).

# Acceptance Criteria

Ctrl/Cmd+[ and ] in Main, All, Thread list, Two columns, a thread (one column and on the right), Settings and Help go back and forward through the places visited, never leaving the app; the thread list and a thread opened from it go back as before; at an end, a status and nothing else; the ? help lists both. Page tests per view with negative controls (bare [ still switches columns; Back at the start does nothing); Chromium at 1280x800 and 393x852 walking back and forward through several views.
