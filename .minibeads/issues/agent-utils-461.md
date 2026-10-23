---
title: 'upstream-link-native: Open in Slack must answer the browser''s own right-click and long-press'
status: open
priority: 2
issue_type: bug
labels:
- vibe-talk
created_at: 2026-10-23T14:54:18.475013663+00:00
updated_at: 2026-10-23T14:54:18.475013663+00:00
---

# Description

[muse watermelon] The owner: "Right now we have an Open in Slack BUTTON in '...'. My whole idea with making it a hyperlink was that I could ALSO right-click it and say Copy link. If it's not a browser-native hyperlink, then I'd like to go back to having 'Copy Slack link' as an additional button. My first choice is a hyperlink if you can make it work; my second choice is two buttons."

#409 open-upstream-link already made the item a real anchor (href, target=_blank, rel=noopener noreferrer), so something keeps the browser's own context menu from it: find it (menu close/gesture handlers, styling, PWA behaviour), fix it so right-click on a desk and long-press on a phone offer the browser's Copy link address, and only where a native link truly cannot work, add "Copy <service> link" as a second item. The l key keeps copying for a keyboard.

# Acceptance Criteria

In real Chromium the menu item is an anchor whose contextmenu event is not prevented and which is still the target when it fires (menu not closed or rebuilt out from under it), at desk and phone sizes; a long press on a phone is not taken by the row's hold gesture. The fake-DOM suite pins the anchor and whatever exemption the fix needs. If a context cannot offer the native menu, a Copy link item stands there instead, saying so.
