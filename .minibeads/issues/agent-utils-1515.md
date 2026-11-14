---
title: 'page-update-reload-loses-controls: pressing New version — reload loses the bottom controls bar'
status: open
priority: 0
issue_type: bug
created_at: 2026-11-14T06:37:00.000000000+00:00
updated_at: 2026-11-14T06:37:00.000000000+00:00
---

# Description

Owner request 051 (11-14 1:37 AM ET), verbatim: "I pressed the chip that said new version reload and it reloaded but with the controls bar at the bottom missing! I saw this before too".

The chip is `#page-update` ("New version — reload", `vibe-talk/web/voice.html`; handler `reloadPage: () => window.location.reload()`). After the reload the bottom controls bar (`#control-pane` in the `#dock`, with `#control-bar`) is missing in the reopened Documents reader, with nothing re-showing it.

# Design

Root cause: `#1505 documents-read-controls-highlight-tap` gated the Documents control pane on a standing document read — `showView` hid it in Documents unless `documentReading` (a playing or pending `doc:` read), and `call.ts renderControls` hid it unless the reader was open AND a read was in progress or pending. A read cannot survive `location.reload()`: the reopened page has no `nowPlaying` and no `pendingRead`, by construction. The Documents reader itself does reopen (deep link `?v=docs&d=KEY`, or a link jump), so every reload from the reader lands on a reader whose transport bar is hidden, and no later event re-shows it — a read starting is the only event that did, through `setReadState`. Reader open/close did not redraw the controls at all since `6773f82d` dropped the `renderControls` call from `onLevelChange`. Owner 049 asked for the reader's controls in every reader state; `#1505` narrowed that to read-standing only.

Checked and cleared: plain channel/call reloads stand the bar up (`showScreen("main")` and the non-Documents branch of `showView`); `Hide reader` (`data-reader`) and keyboard-up (`data-keyboard`) hiding are device choices/transient focus state that reload does not restore; the bar placement is restored from its device key with a bottom default; there is no service worker, and hashed assets are immutable with the shell revalidated, so a deploy swap reloads one consistent build (verified with two built binaries).

Fix: the pane is the reader's transport in every reader state. `renderControls` in Documents hides it only while no reader is open; `showView` keeps it while the reader is open (or a document read stands); the reader's level changes redraw the controls again, so opening, closing and reopening the reader re-decide it. The Documents list keeps no pane.

# Acceptance Criteria

Browser check `vibe-talk/tests/page_update_reload_browser.py` (registered in `vibe-talk/tests.dag.yaml`): at 393x852 and 1280x800, press the chip in the channel and reload at the reader's deep link, asserting the dock and bottom bar are on screen in the call view, the channel (before and after), the Documents list (bar, no pane) and the reader (pane standing before and after the reload, with no read playing). No wall-clock waits. Module test `#1515` in `vibe-talk/tests/js/page/read-aloud.mjs`: a same-storage deep-link reload reopens the reader with the pane and its Read/pace standing and no read surviving; the Documents list reload keeps no pane. The two `#1505` premises that encoded read-standing-only are updated. Validation through the owner fast-track gate.

# Notes

Lane mu-reloadbar. Coordinated by coordb. `#1501` Discuss read-aloud and `#1035` Next-on-last inherit the pane rule unchanged: Discuss is a channel overlay, and a document read standing still keeps the pane through the same reader-open gate.

## Resolution

Unlanded: fix, module test and browser check written in the lane worktree; gate and landing pending.
