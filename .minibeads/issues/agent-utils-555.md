---
title: 'own-row-dark-contrast: reaction counts and edited marks on an unread own row read below contrast in dark mode'
status: open
priority: 3
issue_type: bug
labels:
- vibe-talk
created_at: 2026-10-26T21:09:10.516813060+00:00
updated_at: 2026-10-26T21:09:10.516813060+00:00
---

# Description

[opus 5.5] Found while landing #549 own-read-stored. tests/reactions_browser.py and tests/edited_mark_browser.py measure reaction counts and the edited mark on the owner's rows. They passed because those rows were always greyed (read). Drawn UNREAD (the server not yet having marked them, as the fakes first served them), in dark mode at phone size: "201: ✅1 count reads at 3.52:1", "204: 👀2 / ✅1 at 3.52:1", "a mark reads at 4.12:1". Since #549 an own row can be unread by the owner's choice (put back with Unarchive, or the setting off), so the combination is now ordinary. The browser fakes were changed to serve own rows Done, as the server does, so the checks no longer see it. Wanted: an unread "me" row in dark mode keeps reaction counts and the edited mark at the required contrast, and a browser check covers that state.
