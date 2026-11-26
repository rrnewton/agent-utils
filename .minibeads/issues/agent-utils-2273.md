---
title: 'vibe-talk-cross-view-back-lands-wrong: Back after a citation jump from a thread into All returns to the message and view it came from'
status: open
priority: 1
issue_type: bug
labels:
- vibe-talk
- owner-ui
- ui
created_at: 2026-11-25T20:30:00-05:00
updated_at: 2026-11-25T20:30:00-05:00
---

# Description

A reader in a thread view reads a reply and taps a citation link in it to a main-channel message about a day older, which opens in All. They then press Back. They expect to land back in the thread view, on the reply they came from, at the height it stood. Instead the page shows an unrelated message more than a day older, which sits in a different thread.

The jump crosses views: from the thread view (`v=thread`, the thread's ts and the reply as the focused message) into All (`v=all`, the cited message). The live build already carries the same-view Back fix (#2261 vibe-talk-summary-link-back-race: Back ends the landing it undoes) and the search-query fix (#2251 Back keeps the search query), so this is a different path. The older-page load fix (#2264) is not live yet; the cited message may lie beyond the cached window, so an older-page load can be part of the path.

# Design

Step 1, reproduce on a local test instance (never the live one): thread view, then a citation into All to an older message (also one beyond the cached window), then Back by each means the reader has: browser Back, the phone's system Back, and the in-app Back. Use the Back dump and per-frame Back trace from #2261.

Step 2, root cause before any fix: name what Back restores (the URL's view, thread and message; `history.state`; the saved scroll anchor per view; the thread view's own state) and why it picks a message from another thread (a stale anchor saved for another thread, the All view's anchor applied to the thread view, or an older-page load holding the wrong spot).

Step 3, fix: Back from a cross-view jump returns to the view it left, on the row it left, at that row's height, through the scroll owner.

# Acceptance Criteria

- A browser test through the real server runs thread to All, All to thread, thread to thread and All to All citation jumps, each followed by Back by all three means (browser Back, system Back, in-app Back).
- In every case Back lands in the view the jump left, on the row it left, with a per-frame trace within 1 px where placement is on whole pixels (else 0.5 px), at 412x915 and 1280x800, light and dark.
- The test fails on main where the bug shows, and passes with the fix.
- The test waits on conditions, not wall-clock budgets.
- Before and after evidence on a local test instance, each folder with its BUILD.txt.

# Notes

Owner request 377 (2026-11-25 8:21 PM ET): Back after a citation jump from a thread reply into All landed on an unrelated older message in another thread instead of the reply the reader came from. Owner-UI fast-track, owner tier.

## Root cause (2026-11-25)

Reproduced on a local server (real server binary, fake chat upstream). The message ids in the owner's links decode to a reply at 7:52 PM, its citation target `899` the day before at 4:10 PM, and the landing message 19 minutes older than `899`; the page's own reports for those minutes all say All. So the reader was in All throughout, and the citation was an `:email:` short reference to a message All already drew. (A link copied with Copy vibe-talk link always names a reply's thread, which made the start look like a thread view.)

- `jumpToShortRef` (`web/src/view/shell/navigation.ts`), the `:email:` branch for a message the list on screen already draws (`rowHolding`), landed it with `seekMessage` + `landOnDeepLink` but took no link jump: no spot, no history entry. Back then closed the level below, an EARLIER link or citation jump the reader had never gone back from, and `returnToLinkSpot` put the reader on that jump's spot: an unrelated older message in another thread, with the status "Back where you were." Captured: popstate to the depth-0 entry, the one list write `restoreScroll` < `putBackOnSpot`, the reader's row 5089 px off on a phone and 3633 px on a desk.
- Checked and ruled out: coming back into a thread does run the thread's own first-unread entry landing (`payThreadEntry`), but the return's put-back comes after it and wins; the 9 px a desk walk first showed was a five-row thread whose reader stood within the newest line's slack, so Back took them to the newest line by design.
- Found on the way: the keyboard Back (Ctrl/Cmd+[, `placeFromKeyboard`) walked back a visited place instead of undoing a link jump on top: the thread came back with the jump still standing, "Here is the linked message" (in the first probe the row also stood 51 px off).
- Audit (every entry point that lands on a message): the glossary popover's message source followed its address (a full page load on that message) instead of the app's link jump the glossary panel takes.
- Found in the matrix: Back restored the eye's anchor row (the row at the top of the screen), not the row holding the tapped link, so a Linked mark moving off a row between them shifted the linking row by the mark's height (39 px). A message link or short ref now takes the tapped row as its spot, as a document link already did (`#1082`).
