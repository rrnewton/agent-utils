---
title: 'vibe-talk-summary-strikethrough-live: summary bullet strikethrough must update live on archive, Done or read, and link-follow must always land on screen'
status: closed
priority: 0
issue_type: bug
assignee: mu-strikelive
labels:
- vibe-talk
created_at: 2026-11-18T00:58:00.000000000+00:00
updated_at: 2026-11-18T07:28:00.000000000+00:00
closed_at: 2026-11-18T07:28:00.000000000+00:00
---

# Description

[watermelon-full-preview] Owner request, fast-tracked (owner threads 159-162, 2026-11-17), verbatim: "The strike-through of the bullets in the summary is AWESOME. But it seems flaky... I followed the link to the 1st and the 3rd then archived the message and see no strike through appear. If it is working it should be event driven right? Archiving the message should quickly update the view to show a strikethrough?" / "I clicked to the 3rd message and then clicked back to the 1st and it was NOT on screen ... (and on the 3rd and 4th iteration) it DID appear... one time glitch" / "I refreshed the paged and now my bullets belated turned to strikethrough when I reopened."

The live strike shipped in #1388 discuss-summary-staleness-update restates only from the Discuss panel's own mark buttons, from read-aloud's dismissal wrapper, from two channel-mark wrappers, and on view sync. The message row's own Done/archive act (`createDoneAct` in `vibe-talk/web/src/view/channel/doneact.ts`: `toggleArchived`, `markRowDone`, `dismissMessages`, `restoreMessages`, and `noteArchived` when server read state lands) never restates the strike, so archiving, marking Done or putting back a cited message from its row updates the row at once but leaves the summary bullet unstruck until a refresh redraws the answer. That is the belated strikethrough the owner saw.

Separately, following a summary bullet's citation link lands through `landOnDeepLink` and `revealRow`, which nudges the list once, synchronously. A layout change that lands after that single nudge (the animated Discuss sheet drop, a late row expansion, content above the target growing) leaves the target off screen with nothing re-asserting the landing; on a later identical click everything has settled and it appears. That is the one-time "NOT on screen" glitch.

# Design

Make the strike event-driven on the same state changes that move the message row. `createDoneAct` gains an optional `onReadStateChanged` dep, called at once wherever the act changes the read set it holds: after the optimistic add in `dismissMessages`, after the delete in `restoreMessages`, after a refusal takes either back, and at the end of `noteArchived` when a read (poll, live refresh, another device's mark) lands new server state. The page wires it to the existing `refreshDiscussStrikes`, so every archive, Done, put-back and incoming read restates an open summary's strike in the same task the row changes, with no refresh. The panel's existing restrike on its own marks stays; restriking is idempotent class words only.

Make a link landing stick. `revealRow` (the one landing every link, reply arrow and sent-message jump shares) re-asserts its landing over the next animation frames: while the row stands off screen and layout may still be settling, it nudges again from the row's current position; it stops as soon as the row is on screen, the row leaves the list, or the reader scrolls. No wall-clock budget: each frame is the condition check.

Scope is summary bullets and link-follow only. Discuss persistence across refresh is #1821 vibe-talk-discuss-persistence (not yet filed here), owned elsewhere.

# Acceptance Criteria

- Archiving, marking Done, putting back, or reading (including read state arriving from the server) a message a summary bullet cites updates that bullet's strikethrough at once, with no page refresh, in the same turn the message row changes.
- A bullet whose cited messages are not all read is never struck; putting a cited message back un-strikes it live.
- Following a bullet's link always brings the cited message on screen, on repeated follows and when layout shifts just after the landing.
- Deterministic page tests (owner rule 346: condition-based waits only, no wall-clock assertions) prove: archive gives a strikethrough without reload, and repeated link following always lands on screen, at phone and desktop sizes.
- Screenshot matrix on a test instance per the standing owner-UI acceptance: light and dark theme, on-screen keyboard open and closed, phone 412x915 and desktop 1280x800, Discuss half-pane open in at least the phone shots, plus the struck and unstruck states before and after.

# Notes

Owner fast-track, deployed on landing (owner-priority gate). Sibling: #1821 owns Discuss persistence across refresh; this issue is only the live strike and the link landing.

## Resolution

[watermelon-full-preview] Landed on github/main as `2674b6717e1680471b5a9ee0da027aa1b9670c17` (gated head `f0f4f704`; the gate's one failure, `vibe-talk.check.search-unmatched-browser`, passed 3/3 standalone on both this tree and current main and passed its solo gate rerun). `createDoneAct` now notifies `onReadStateChanged` at once on a Done, a put back, a refused write's take-back, and when server read state lands in `noteArchived`; the page wires it to `refreshDiscussStrikes`, so archiving or marking Done from a cited message's own row strikes its summary bullet in the same turn the row greys, with no refresh. `revealRow` re-asserts its landing over the next animation frames while the row stands off screen, so a landing moved by late layout is brought back. Tests: unit coverage in `tests/js/view/channel.test.mjs`, the new `tests/discuss_strike_live_browser.py` (repeated follows land on screen; archive, put back and re-archive from the rows strike and un-strike without reload, at 412x915 and 1280x800), and a 16-image screenshot matrix (light and dark, keyboard open with a real `visualViewport` shrink and closed, phone and desktop, before and after) reviewed by the coordinator liaison.
