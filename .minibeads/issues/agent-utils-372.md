---
title: 'settings-header-layout: the server line, Expand all and Open help share one quiet row at the head of Settings'
status: closed
priority: 2
issue_type: bug
labels:
- vibe-talk
created_at: 2026-10-19T14:44:35.378228772+00:00
updated_at: 2026-10-19T15:20:04.195725784+00:00
closed_at: 2026-10-19T15:20:04.195725323+00:00
---

# Description

[opus 5.5] On a desktop window the head of Settings wastes rows and shouts. The server host is drawn in a large bold monospace font that wraps over two or three lines in its own card; "Expand all" stands alone on its own row above the sections; and "Open help" stands alone on its own row below them.

What is wanted:

1. The server host is quiet secondary information: small, normal-weight text on one line, cut with an ellipsis when it does not fit, with the full value in its tooltip, and still readable and copyable.
2. "Expand all" / "Fold all" does not take a row of its own: it sits on the same row as the server line, at its end.
3. "Open help" does not take a row of its own either: it sits beside Expand all.
4. On a phone (393x852) the head stays sensible: the host on one line, the two buttons together under it, nothing running off the screen.
5. Keyboard and screen-reader order stay logical: Back, the server line, Expand all, Open help, then the sections in their order.
6. The keyboard walk and the screenshot scenes still pass, and the Settings batch behaviour (#345 settings-multi-column, #346 settings-collapsible, #347 settings-prose, #348 settings-dynamic-lists, #349 channel-settings-all, #351 settings-polish) is unchanged.

Page tests that fail before the change, plus Chromium screenshots before and after at 1280x800, 1920x1080 and 393x852.
