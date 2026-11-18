---
title: 'vibe-talk-discuss-state-persists: Discuss open, size and scope survive a refresh'
status: closed
priority: 0
issue_type: bug
labels:
- vibe-talk
created_at: 2026-11-17T20:01:58-05:00
updated_at: 2026-11-18T00:08:07-05:00
closed_at: 2026-11-18T00:08:07-05:00
---

# Description

[watermelon-full-preview] Owner request, fast-tracked (owner thread 162), verbatim: "Refresh LOSES the state of the discuss panel... it does not keep it open." A page refresh closes the Discuss panel and forgets which conversation it showed, so the reader must reopen it and re-find its scope on every reload.

# Design

- Persist per device in local storage: whether Discuss stands open, the conversation it shows (channel plus thread, or the channel's main/flat view), alongside the already-kept docked size (overlay width, docked height, sheet height and layout mode), which this change leaves as it finds it.
- Keep the persisted state in one device setting beside the other Discuss settings; a damaged entry reads as closed, and a refused read is closed.
- Write the state from the one place openness changes, so every exit (bar toggle, Close, Escape, scope-binding close) persists as closed and every open persists its bound scope.
- Restore on load without a flash of the closed state where possible: reopen the panel for its kept scope (preview and saved record only, no model call) during the boot, before the main screen is shown, once the page's channel matches; a kept scope on another channel does not force the panel open.
- Persistence only: the panel title and scoping behaviour belong to a sibling change, and Documents gating to another; neither is touched here.

# Acceptance Criteria

- Open Discuss, reload, and it is still open showing the same scope (main or thread); close it, reload, and it stays closed.
- The kept docked size (width, docked height, sheet height, layout mode) still stands after a reload with the panel open.
- A damaged or refused persisted entry leaves the panel closed and never throws.
- Page tests at phone and desktop sizes cover open-then-reload and close-then-reload in light and dark theme, with condition-based waits only.

# Notes

Owner fast-track (owner thread 162). Owner-UI screenshot matrix applies: light and dark theme, keyboard open and closed, phone 412x915 and desktop 1280x800 on a test instance, with the before/after persisted state shown.

## Resolution

[watermelon-full-preview] Landed on github/main as `d3818f6d091056378e8b4d56a86ac3aa7d9ced3f` (gated head `be91a6f4`, gate green; combined tree `4f23c158` verified after main moved). Discuss openness and its bound scope (channel plus thread, or the channel's main/flat view) now persist per device in `vibe-talk.voice.discuss-panel` beside the already-kept docked size, written from the panel's one openness change so every close persists as closed, and restored during boot (preview and saved record only, no model call) once the kept channel is on screen; another channel's kept scope, a busy panel, or a not-yet-known summarizer defers the restore instead of forcing it. Unit tests cover keep-open, keep-closed, restore, and the other-channel case; the Chromium walk and the screenshot matrix (light/dark, keyboard open/closed, phone 412x915 at scale 2.6 and desktop 1280x800, before/after reload) show open-after-reload with the same scope and closed-after-reload closed, on a test instance with a fake API throughout.
