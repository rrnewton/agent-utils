---
title: 'vibe-talk-composer-stays-in-view-while-dictating: keep the focused composer on screen during live refresh'
status: open
priority: 0
issue_type: bug
labels:
- vibe-talk
- ui
- scroll
created_at: 2026-11-18T08:15:00-05:00
updated_at: 2026-11-18T08:15:00-05:00
---

# Description

[watermelon-full-preview] Owner report (thread 171, 2026-11-18 08:09 ET): while dictating into the text box inside a thread, the view keeps jumping up into the scrollback instead of staying on the composer. Scroll traces from the owner's page (13:09Z) show focus stays on `channel-compose-text` with the keyboard open (visual viewport 579px), but a `placeDrillAnchor`/`payThreadEntry` nudge writes scrollTop 3508 -> 1291 to hold a message anchor, leaving the composer at y=2617..2787 - about 2000px below the visible viewport - while `toFoot` writes pull the opposite way. Just before, a live refresh re-renders rows (a burst in which every `#discord-log` LI is removed and added, scroll height 3787 -> 4060; 924 mutate entries since 12:00Z).

# Design

- While a composer (channel or thread) has focus, or input arrived within the last few seconds, scroll holders anchor to the composer, not a message: no anchor-hold, drill-anchor keeper, or foot-follow write may move the composer off screen, and after a mutation the composer is returned to the visual viewport with one measured write (no smooth glide through history).
- The drill-in anchor keeper (#1818) stands down while the composer is active instead of re-placing its message on every list mutation.
- Stop the needless full-row re-render on live refresh where it is a regression: keyed reuse for the thread list, and row identity that survives a word-for-word identical refresh for the channel list.
- A deterministic browser test (condition waits only): keyboard really shrunk, thread composer focused, dictation simulated with input events while live updates and a full refresh arrive; the composer stays fully inside the visual viewport and scrollTop never jumps.

# Acceptance Criteria

- With the thread composer focused and dictation input arriving, live updates and a full refresh never move the composer outside the visual viewport and never animate scrollTop through history.
- The drill-anchor and message-anchor holds resume their normal behaviour once the composer is no longer focused and input has been quiet.
- A live refresh that changes nothing does not replace every row in `#discord-log` or `#thread-list`.
- The deterministic browser test passes at phone size with the keyboard open, and the screenshot matrix (light/dark, keyboard open/closed, phone/desktop, Discuss half-pane on phone, before/after) is captured on a test instance and inspected.

# Notes

Root cause reported to coordb before the fix (2026-11-18): `placeDrillAnchor` in `vibe-talk/web/src/view/channel/threadentry.ts` keeps a clicked message at a fixed viewport top through ResizeObserver/MutationObserver re-placement with no composer-focus stand-down, and `holdingRows`/`restoreScroll` in `scrollkeep.ts` always restore to a message anchor; the two fight `toFoot` while the owner dictates. `#thread-list` is rebuilt with `replaceChildren(...threadCards(...))` on every landing in `readcore.ts`.
