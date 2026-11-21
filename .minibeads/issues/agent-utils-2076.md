---
title: 'vibe-talk-top-anchor-policy: user-caused height changes keep the touched message''s top still, even at the end of history'
status: open
priority: 1
issue_type: bug
labels:
- vibe-talk
- owner-ui
created_at: 2026-11-21T14:39:00-05:00
updated_at: 2026-11-21T14:39:00-05:00
---

# Description

The owner: "Clicking either in the beginning or end of the message keeps the BOTTOM of the message fixed and the messages ABOVE it move... why is that? What is the policy? In this case we are scrolled to the END of the message history with the text input window on the bottom of the screen. Still, I would rather in this case the top of the message I click stay FIXED and the area below MOVE. I want to bias it towards static top / dynamic bottom wherever that is possible. Even if that pulls UP the message-send text box so it is no longer at the BOTTOM of the screen."

# Design

One anchoring rule, in one place: for any user-caused height change (selecting or expanding a message, fold or unfold, action row, quote, reply box), the top of the touched message stays where it is and everything below moves, including at the end of history, even if the composer lifts off the bottom. Stick-to-bottom applies only to newly arriving messages. Record the policy in the view-stability design notes.

# Acceptance Criteria

- The touched message's top offset changes by 0 px in every frame for each user-caused height change, at the end of history and mid-list, on phone and desktop, windowed on and off.
- Arriving messages still stick to the bottom when the view is at the end.
- Deterministic frame- and condition-based browser checks; screenshots.

# Notes

Filed by the coordinator so the issue link resolves on main; the owning lane refines this file as it works.

Refined by mu-topanchor (2026-11-21). What pinned the bottom, in the one scroll owner
(`web/src/view/channel/scrollkeep.ts`): (1) `captureScroll` marks a reader at the foot
`pinned`, and `restoreScroll` answered every such change with `scrollToNewest`, so any
redraw under a click re-pinned the list's bottom; (2) `preservingCollapse` had an
explicit foot branch (`scrollTop = scrollHeight`) for folds; (3) the `listkeepers.ts`
ResizeObserver foot keepers re-asserted the foot after async resizes. Native anchoring
is off (`overflow-anchor: none`), so this logic was the only anchor. The windowed list
only amplifies (3) through re-slices. Baseline, rAF-sampled on a 90-message synthetic
channel, expand at the end: touched top -226px (412px phone) / -104px (1280px), bottom
within 0.6px, windowed on and off; mid-list the top already held at 0px.

The fix, one rule in the scroll owner, scoped to the channel lists (the call
transcript keeps its own tapped-card following contract, `#431`): only the acts that
change a message's height open a hold — `preservingCollapse` in a channel list,
`beginUserChange` from the fold steps and the details sheet — never a navigation key
or a selection press. Its marks read `userCaused` and never take the pinned restore,
`preservingCollapse` has no foot branch in a channel list, the foot keepers skip their
write while the hold stands, and a shrink at the very foot whose top must hold stands
in room the owner holds past the foot (counted by the foot measures, reclaimed by the
reader's next scroll or deliberate write to the foot). `pinned` survives only on the
arrival path (`followIfPinned`), itself gated by the hold in the channel view: an
arrival landing while an act settles (or whose capture predates the act) is counted on
the Newest chip and never undoes the anchor, with no retroactive follow — only a later
arrival whose own capture finds the reader at the foot sticks. The quote/reply box is
`#2063`'s dock (the box is painted at the pane foot, the list unmoved); this hold is
what keeps the keepers from dragging the list while that dock settles. Policy recorded
in `vibe-talk/README.md`, "Nothing moves the list but the reader". Checks:
`tests/top_anchor_browser.py` (DAG step `top-anchor-browser`, 178) plus scroll-owner
unit cases in `tests/js/view/channel.test.mjs`. After the fix every measured cell is
0.0-0.3px.
