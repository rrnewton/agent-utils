---
title: 'vibe-talk-reload-keeps-view-and-place: a reload keeps the view, the linked message and the scroll anchor'
status: open
priority: 0
issue_type: bug
labels:
- vibe-talk
- ui
- scroll
created_at: 2026-11-18T09:00:00-05:00
updated_at: 2026-11-18T09:00:00-05:00
---

# Description

[watermelon-full-preview] Owner report (thread 187): "I'm still seeing problems where a reload of a page loses my place. Most recently I did a search, clicked some message, ended up in THREAD LIST view with a particular thread highlighted and its replies grouped under it. Either way when I did a refresh I was back at the bottom of the Main view. I do notice that it is stable when I am drilled into a thread. However I guess if there is not a linked message selected... we don't store anything about scroll position in the URL (and shouldn't!) so maybe I should of course expect to be at a different scroll position, but I was hoping to remain in the same view."

What the liaison committed to the owner: (1) a reload keeps the VIEW (Main / All / Threads) and the channel; it apparently falls back to Main now, so treat that as a bug and find why (URL state? saved view? a search-landing path that doesn't record the view?); (2) the plain scroll position stays OUT of the URL but is saved in per-tab sessionStorage, anchored to the top-visible message id plus an offset, and restored on reload. Scope correction (owner, thread 188): search only filters the current view; a message is opened only by clicking its Thread button to drill in, which already survives a reload — so no search-to-linked-message change. Whether an active search filter should also survive a reload is pending with the owner.

Coordinate: scroll restoration must go through the single scroll-anchoring authority (scrollkeep; #1839 composer guard just landed as 59651b9a; #1846 windowed rendering is designing one authority). Don't add a new scroll writer; restore through scrollkeep's owner.

# Design

- The Threads (thread list) view is a view a channel reopens in, like Main, All and a thread: it is recorded as the channel's chosen view, honoured on reopen and from a deep link (`v=threads`), and shown on reopen. The channel is kept as it already is.
- The plain scroll place is kept in per-tab sessionStorage (the window's own UI-state record), anchored to the top-visible message id plus its offset from the list head, measured through scrollkeep's `captureScroll` and put back through its `restoreScroll`. It is captured as the reader scrolls (throttled, only when the anchor changes), not only when the page hides, and never written to the URL.

# Acceptance Criteria

- Reloading in Main, All or Threads (thread list), on a threaded channel and on a channel without threads (Main only), comes back in the same view on the same channel.
- A reload with no linked message keeps the view and restores the session scroll anchor (top-visible message id plus offset); the scroll position itself never appears in the URL.
- A deterministic browser test reloading after a search filter in each view (Main, All, Threads), asserting view and scroll anchor survive, plus a reload with no linked message; screenshot matrix (light/dark, keyboard open/closed, phone 412x915 and desktop 1280x800, Discuss half-pane on phone, before/after) captured on a test instance and inspected.

# Notes

Owner-priority gate, deployed on landing. Scroll work goes through scrollkeep's owner only; no new scroll writer.
